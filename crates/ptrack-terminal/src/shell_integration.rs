use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::profile::{Profile, ProfileKind};

const NONCE_ENVIRONMENT: &str = "PTRACK_SHELL_INTEGRATION_NONCE_V1";
const WRAPPER_ENVIRONMENT: &str = "PTRACK_SHELL_INTEGRATION_WRAPPER_V1";
const ORIGINAL_ZDOTDIR_ENVIRONMENT: &str = "PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1";
const MAX_HOOK_BYTES: usize = 16 * 1_024;
static OWNER_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellIntegrationQuality {
    None,
    Rich,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellIntegrationDescriptor {
    pub quality: ShellIntegrationQuality,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nonce: String,
}

impl ShellIntegrationDescriptor {
    /// Returns the descriptor used for an unmodified shell launch.
    #[must_use]
    pub fn none() -> Self {
        Self {
            quality: ShellIntegrationQuality::None,
            nonce: String::new(),
        }
    }
}

#[derive(Debug)]
pub struct ShellIntegrationError {
    context: &'static str,
    source: io::Error,
}

impl ShellIntegrationError {
    fn new(context: &'static str, source: io::Error) -> Self {
        Self { context, source }
    }
}

impl fmt::Display for ShellIntegrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for ShellIntegrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
pub struct ShellIntegrationOwner {
    directory: PathBuf,
    bash_regular: PathBuf,
}

impl ShellIntegrationOwner {
    /// Creates one manager-lifetime private hook directory when needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the private directory or bounded hook files cannot
    /// be created and synchronized.
    pub fn new<'a>(
        profiles: impl IntoIterator<Item = &'a Profile>,
    ) -> Result<Option<Self>, ShellIntegrationError> {
        Self::new_for_os(profiles, std::env::consts::OS)
    }

    pub(crate) fn new_for_os<'a>(
        profiles: impl IntoIterator<Item = &'a Profile>,
        os: &str,
    ) -> Result<Option<Self>, ShellIntegrationError> {
        if os == "windows"
            || !profiles
                .into_iter()
                .any(|profile| supports_shell_integration_for_os(profile, os))
        {
            return Ok(None);
        }
        let directory = create_private_owner_directory().map_err(|error| {
            ShellIntegrationError::new("create shell integration directory", error)
        })?;
        let bash_regular = directory.join("bash-regular");
        let files = BTreeMap::from([
            (bash_regular.clone(), BASH_REGULAR_INTEGRATION.to_owned()),
            (
                directory.join(".zshenv"),
                ZSH_ENVIRONMENT_INTEGRATION.to_owned(),
            ),
            (
                directory.join(".zprofile"),
                format!(
                    "{}{}",
                    zsh_startup_source(".zprofile"),
                    ZSH_RESTORE_WRAPPER_DIRECTORY
                ),
            ),
            (
                directory.join(".zshrc"),
                format!("{}{}", zsh_startup_source(".zshrc"), ZSH_HOOK_INTEGRATION),
            ),
            (directory.join(".zlogin"), zsh_startup_source(".zlogin")),
            (directory.join(".zlogout"), zsh_startup_source(".zlogout")),
        ]);
        for (path, contents) in files {
            if contents.len() > MAX_HOOK_BYTES {
                let error = io::Error::new(io::ErrorKind::InvalidData, "shell hook is too large");
                let _ = fs::remove_dir_all(&directory);
                return Err(ShellIntegrationError::new("write shell integration", error));
            }
            if let Err(error) = write_private_file(&path, contents.as_bytes()) {
                let _ = fs::remove_dir_all(&directory);
                return Err(ShellIntegrationError::new("write shell integration", error));
            }
        }
        Ok(Some(Self {
            directory,
            bash_regular,
        }))
    }

    /// Returns the private manager-lifetime hook directory.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Reports whether this owner can enrich the profile without changing its semantics.
    #[must_use]
    pub fn supports(&self, profile: &Profile) -> bool {
        supports_shell_integration(profile)
    }

    /// Returns independently owned launch arguments, environment, and descriptor.
    #[must_use]
    pub fn prepare(
        &self,
        profile: &Profile,
        environment: &[String],
        nonce: &str,
    ) -> (Vec<String>, Vec<String>, ShellIntegrationDescriptor) {
        let mut args = profile.args.clone();
        let mut env = environment.to_vec();
        if !self.supports(profile) || nonce.is_empty() {
            return (args, env, ShellIntegrationDescriptor::none());
        }
        let executable = executable_basename(&profile.executable);
        if executable.eq_ignore_ascii_case("zsh") {
            if !supported_interactive_shell_args(&args) {
                return (args, env, ShellIntegrationDescriptor::none());
            }
            let mut original_zdotdir = environment_value(&env, "ZDOTDIR").unwrap_or_default();
            if original_zdotdir.is_empty() {
                original_zdotdir = environment_value(&env, "HOME").unwrap_or_default();
            }
            env = set_environment_value(env, NONCE_ENVIRONMENT, nonce);
            env = set_environment_value(
                env,
                WRAPPER_ENVIRONMENT,
                &self.directory().to_string_lossy(),
            );
            env = set_environment_value(env, ORIGINAL_ZDOTDIR_ENVIRONMENT, &original_zdotdir);
            env = set_environment_value(env, "ZDOTDIR", &self.directory().to_string_lossy());
            return (
                args,
                env,
                ShellIntegrationDescriptor {
                    quality: ShellIntegrationQuality::Rich,
                    nonce: nonce.to_owned(),
                },
            );
        }
        if executable.eq_ignore_ascii_case("bash") {
            let (supported, login) = supported_bash_args(&args);
            if !supported || login {
                return (args, env, ShellIntegrationDescriptor::none());
            }
            args = vec![
                "--init-file".to_owned(),
                self.bash_regular.to_string_lossy().into_owned(),
                "-i".to_owned(),
            ];
            env = set_environment_value(env, NONCE_ENVIRONMENT, nonce);
            return (
                args,
                env,
                ShellIntegrationDescriptor {
                    quality: ShellIntegrationQuality::Rich,
                    nonce: nonce.to_owned(),
                },
            );
        }
        (args, env, ShellIntegrationDescriptor::none())
    }

    /// Removes the manager-lifetime hook directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the hook directory cannot be removed.
    pub fn close(mut self) -> Result<(), ShellIntegrationError> {
        self.remove_directory().map_err(|error| {
            ShellIntegrationError::new("remove shell integration directory", error)
        })
    }

    fn remove_directory(&mut self) -> io::Result<()> {
        match fs::remove_dir_all(&self.directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl Drop for ShellIntegrationOwner {
    fn drop(&mut self) {
        let _ = self.remove_directory();
    }
}

/// Applies shell integration or returns an explicitly unmodified launch.
#[must_use]
pub fn prepare_shell_integration(
    owner: Option<&ShellIntegrationOwner>,
    profile: &Profile,
    environment: &[String],
    nonce: &str,
) -> (Vec<String>, Vec<String>, ShellIntegrationDescriptor) {
    owner.map_or_else(
        || {
            (
                profile.args.clone(),
                environment.to_vec(),
                ShellIntegrationDescriptor::none(),
            )
        },
        |owner| owner.prepare(profile, environment, nonce),
    )
}

/// Reports whether shell integration preserves this launch's semantics.
#[must_use]
pub fn supports_shell_integration(profile: &Profile) -> bool {
    supports_shell_integration_for_os(profile, std::env::consts::OS)
}

fn supports_shell_integration_for_os(profile: &Profile, os: &str) -> bool {
    if os == "windows" || profile.kind != ProfileKind::Shell {
        return false;
    }
    let executable = executable_basename(&profile.executable);
    if executable.eq_ignore_ascii_case("zsh") {
        return supported_interactive_shell_args(&profile.args);
    }
    if executable.eq_ignore_ascii_case("bash") {
        let (supported, login) = supported_bash_args(&profile.args);
        return supported && !login;
    }
    false
}

fn executable_basename(executable: &str) -> &str {
    executable.rsplit(['/', '\\']).next().unwrap_or(executable)
}

fn supported_interactive_shell_args(args: &[String]) -> bool {
    args.iter().all(|argument| {
        if matches!(argument.as_str(), "--login" | "--interactive") {
            return true;
        }
        if !argument.starts_with('-') || matches!(argument.as_str(), "-" | "--") {
            return false;
        }
        argument
            .strip_prefix('-')
            .expect("dash prefix checked")
            .chars()
            .all(|flag| matches!(flag, 'l' | 'i'))
    })
}

fn supported_bash_args(args: &[String]) -> (bool, bool) {
    if !supported_interactive_shell_args(args) {
        return (false, false);
    }
    let login = args.iter().any(|argument| {
        argument == "--login"
            || argument
                .strip_prefix('-')
                .is_some_and(|flags| flags.contains('l'))
    });
    (true, login)
}

fn environment_value(environment: &[String], key: &str) -> Option<String> {
    environment.iter().find_map(|entry| {
        let (name, value) = entry.split_once('=')?;
        (name == key).then(|| value.to_owned())
    })
}

fn set_environment_value(environment: Vec<String>, key: &str, value: &str) -> Vec<String> {
    let mut result = Vec::with_capacity(environment.len() + 1);
    let mut replaced = false;
    for entry in environment {
        let matches = entry.split_once('=').is_some_and(|(name, _)| name == key);
        if matches {
            if !replaced {
                result.push(format!("{key}={value}"));
                replaced = true;
            }
        } else {
            result.push(entry);
        }
    }
    if !replaced {
        result.push(format!("{key}={value}"));
    }
    result
}

fn create_private_owner_directory() -> io::Result<PathBuf> {
    for _ in 0..128 {
        let sequence = OWNER_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ptrack-shell-integration-{}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&directory) {
            Ok(()) => {
                #[cfg(unix)]
                set_unix_mode(&directory, 0o700)?;
                return Ok(directory);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "shell integration directory name collisions",
    ))
}

fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    set_unix_mode(path, 0o600)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(unix)]
fn set_unix_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

fn zsh_startup_source(name: &str) -> String {
    format!(
        r#"if [[ -n "${{PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1:-}}" && "${{PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}}" != "${{PTRACK_SHELL_INTEGRATION_WRAPPER_V1}}" && -r "${{PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}}/{name}" ]]; then
  source "${{PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}}/{name}"
fi
"#
    )
}

const ZSH_ENVIRONMENT_INTEGRATION: &str = r#"if [[ -n "${PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1:-}" && "${PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}" != "${PTRACK_SHELL_INTEGRATION_WRAPPER_V1}" && -r "${PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}/.zshenv" ]]; then
  source "${PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}/.zshenv"
fi
export ZDOTDIR="${PTRACK_SHELL_INTEGRATION_WRAPPER_V1}"
"#;

const ZSH_RESTORE_WRAPPER_DIRECTORY: &str =
    "export ZDOTDIR=\"${PTRACK_SHELL_INTEGRATION_WRAPPER_V1}\"\n";

const ZSH_HOOK_INTEGRATION: &str = r#"
typeset -g __ptrack_shell_nonce="${PTRACK_SHELL_INTEGRATION_NONCE_V1:-}"
unset PTRACK_SHELL_INTEGRATION_NONCE_V1
typeset -gi __ptrack_shell_started=0
typeset -g __ptrack_shell_original_ps1=""
typeset -g __ptrack_shell_wrapped_ps1=""
function __ptrack_shell_encoded_cwd() {
  local LC_ALL=C
  local __ptrack_value="$PWD"
  local __ptrack_encoded=""
  local __ptrack_character __ptrack_hex
  integer __ptrack_index __ptrack_code
  for (( __ptrack_index=1; __ptrack_index <= ${#__ptrack_value}; __ptrack_index++ )); do
    __ptrack_character="${__ptrack_value[__ptrack_index]}"
    case "$__ptrack_character" in
      [A-Za-z0-9/._~-]) __ptrack_encoded+="$__ptrack_character" ;;
      *) printf -v __ptrack_code '%d' "'$__ptrack_character"; (( __ptrack_code &= 255 )); printf -v __ptrack_hex '%%%02X' "$__ptrack_code"; __ptrack_encoded+="$__ptrack_hex" ;;
    esac
    (( ${#__ptrack_encoded} <= 4000 )) || return 1
  done
  printf '%s' "$__ptrack_encoded"
}
function __ptrack_shell_precmd() {
  local __ptrack_status=$?
  local __ptrack_cwd __ptrack_prompt_end
  if (( __ptrack_shell_started )); then
    printf '\033]133;D;%d\007' "$__ptrack_status"
    printf '\033]633;D;%d;%s\007' "$__ptrack_status" "$__ptrack_shell_nonce"
  fi
  if __ptrack_cwd="$(__ptrack_shell_encoded_cwd)"; then
    printf '\033]7;file://%s\007' "$__ptrack_cwd"
    printf '\033]633;P;Cwd=file://%s;%s\007' "$__ptrack_cwd" "$__ptrack_shell_nonce"
  fi
  if [[ "$PS1" != "$__ptrack_shell_wrapped_ps1" ]]; then
    __ptrack_shell_original_ps1="$PS1"
  fi
  printf -v __ptrack_prompt_end '\033]133;B\007\033]633;B;%s\007' "$__ptrack_shell_nonce"
  __ptrack_shell_wrapped_ps1="${__ptrack_shell_original_ps1}%{${__ptrack_prompt_end}%}"
  PS1="$__ptrack_shell_wrapped_ps1"
  printf '\033]133;A\007\033]633;A;%s\007' "$__ptrack_shell_nonce"
}
function __ptrack_shell_preexec() {
  __ptrack_shell_started=1
  printf '\033]133;C\007\033]633;C;%s\007' "$__ptrack_shell_nonce"
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __ptrack_shell_precmd
add-zsh-hook preexec __ptrack_shell_preexec
export ZDOTDIR="${PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}"
"#;

const BASH_REGULAR_INTEGRATION: &str = r#"if [[ -r ~/.bashrc ]]; then source ~/.bashrc; fi

__ptrack_shell_nonce="${PTRACK_SHELL_INTEGRATION_NONCE_V1:-}"
unset PTRACK_SHELL_INTEGRATION_NONCE_V1
__ptrack_shell_started=0
__ptrack_shell_in_command=1
__ptrack_original_prompt_command=("${PROMPT_COMMAND[@]}")
__ptrack_shell_original_ps1=""
__ptrack_shell_wrapped_ps1=""
__ptrack_restore_status() {
  return "$1"
}
__ptrack_shell_encoded_cwd() {
  local LC_ALL=C
  local __ptrack_value="$PWD"
  local __ptrack_encoded=""
  local __ptrack_character __ptrack_hex __ptrack_index __ptrack_code
  for (( __ptrack_index=0; __ptrack_index < ${#__ptrack_value}; __ptrack_index++ )); do
    __ptrack_character="${__ptrack_value:$__ptrack_index:1}"
    case "$__ptrack_character" in
      [A-Za-z0-9/._~-]) __ptrack_encoded+="$__ptrack_character" ;;
      *) printf -v __ptrack_code '%d' "'$__ptrack_character"; (( __ptrack_code &= 255 )); printf -v __ptrack_hex '%%%02X' "$__ptrack_code"; __ptrack_encoded+="$__ptrack_hex" ;;
    esac
    (( ${#__ptrack_encoded} <= 4000 )) || return 1
  done
  printf '%s' "$__ptrack_encoded"
}
__ptrack_shell_prompt() {
  local __ptrack_status=$?
  local __ptrack_prompt_command __ptrack_cwd __ptrack_prompt_end
  for __ptrack_prompt_command in "${__ptrack_original_prompt_command[@]}"; do
    if [[ -n "$__ptrack_prompt_command" ]]; then
      __ptrack_restore_status "$__ptrack_status"
      eval "$__ptrack_prompt_command"
    fi
  done
  if [[ "$__ptrack_shell_started" == 1 ]]; then
    printf '\033]133;D;%d\007' "$__ptrack_status"
    printf '\033]633;D;%d;%s\007' "$__ptrack_status" "$__ptrack_shell_nonce"
  fi
  if __ptrack_cwd="$(__ptrack_shell_encoded_cwd)"; then
    printf '\033]7;file://%s\007' "$__ptrack_cwd"
    printf '\033]633;P;Cwd=file://%s;%s\007' "$__ptrack_cwd" "$__ptrack_shell_nonce"
  fi
  if [[ "$PS1" != "$__ptrack_shell_wrapped_ps1" ]]; then
    __ptrack_shell_original_ps1="$PS1"
  fi
  printf -v __ptrack_prompt_end '\033]133;B\007\033]633;B;%s\007' "$__ptrack_shell_nonce"
  __ptrack_shell_wrapped_ps1="${__ptrack_shell_original_ps1}\\[${__ptrack_prompt_end}\\]"
  PS1="$__ptrack_shell_wrapped_ps1"
  printf '\033]133;A\007\033]633;A;%s\007' "$__ptrack_shell_nonce"
  __ptrack_shell_started=1
  __ptrack_shell_in_command=0
}
__ptrack_get_debug_trap() {
  local -a __ptrack_trap_terms
  eval "__ptrack_trap_terms=( $(trap -p DEBUG) )"
  printf '%s' "${__ptrack_trap_terms[2]:-}"
}
__ptrack_original_debug_trap="$(__ptrack_get_debug_trap)"
__ptrack_shell_preexec() {
  if [[ "$__ptrack_shell_in_command" == 0 ]]; then
    __ptrack_shell_in_command=1
    printf '\033]133;C\007\033]633;C;%s\007' "$__ptrack_shell_nonce"
  fi
  if [[ -n "$__ptrack_original_debug_trap" ]]; then
    eval "$__ptrack_original_debug_trap"
  fi
}
trap '__ptrack_shell_preexec' DEBUG
PROMPT_COMMAND=__ptrack_shell_prompt
"#;
