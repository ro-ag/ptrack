package terminal

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

type ShellIntegrationQuality string

const (
	ShellIntegrationNone ShellIntegrationQuality = "none"
	ShellIntegrationRich ShellIntegrationQuality = "rich"
)

// ShellIntegrationDescriptor is process-local renderer metadata. Nonce is a
// correlation value, not capability authority, and must never be persisted or
// logged. A child shell cannot use it outside terminal presentation handling.
type ShellIntegrationDescriptor struct {
	Quality ShellIntegrationQuality `json:"quality"`
	Nonce   string                  `json:"nonce,omitempty"`
}

const (
	shellIntegrationNonceEnvironment   = "PTRACK_SHELL_INTEGRATION_NONCE_V1"
	shellIntegrationWrapperEnvironment = "PTRACK_SHELL_INTEGRATION_WRAPPER_V1"
	shellIntegrationOriginalZDOTDIR    = "PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1"
)

type shellIntegrationOwner struct {
	directory   string
	bashRegular string
}

func newShellIntegrationOwner(profiles map[string]Profile) (*shellIntegrationOwner, error) {
	if runtime.GOOS == "windows" || !hasSupportedIntegratedShell(profiles) {
		return nil, nil
	}
	directory, err := os.MkdirTemp("", "ptrack-shell-integration-")
	if err != nil {
		return nil, fmt.Errorf("create shell integration directory: %w", err)
	}
	owner := &shellIntegrationOwner{
		directory:   directory,
		bashRegular: filepath.Join(directory, "bash-regular"),
	}
	cleanup := func(writeErr error) (*shellIntegrationOwner, error) {
		return nil, errors.Join(writeErr, os.RemoveAll(directory))
	}
	files := map[string]string{
		owner.bashRegular:                     bashRegularIntegration,
		filepath.Join(directory, ".zshenv"):   zshEnvironmentIntegration,
		filepath.Join(directory, ".zprofile"): zshStartupSource(".zprofile") + zshRestoreWrapperDirectory,
		filepath.Join(directory, ".zshrc"):    zshStartupSource(".zshrc") + zshHookIntegration,
		filepath.Join(directory, ".zlogin"):   zshStartupSource(".zlogin"),
		filepath.Join(directory, ".zlogout"):  zshStartupSource(".zlogout"),
	}
	for path, contents := range files {
		if err := os.WriteFile(path, []byte(contents), 0o600); err != nil {
			return cleanup(fmt.Errorf("write shell integration: %w", err))
		}
	}
	return owner, nil
}

func hasSupportedIntegratedShell(profiles map[string]Profile) bool {
	for _, profile := range profiles {
		if supportsShellIntegration(profile) {
			return true
		}
	}
	return false
}

func (owner *shellIntegrationOwner) prepare(
	profile Profile,
	environment []string,
	nonce string,
) ([]string, []string, ShellIntegrationDescriptor) {
	args := append([]string(nil), profile.Args...)
	env := append([]string(nil), environment...)
	if !owner.supports(profile) || nonce == "" {
		return args, env, ShellIntegrationDescriptor{Quality: ShellIntegrationNone}
	}
	switch strings.ToLower(filepath.Base(profile.Executable)) {
	case "zsh":
		if !supportedInteractiveShellArgs(args) {
			return args, env, ShellIntegrationDescriptor{Quality: ShellIntegrationNone}
		}
		originalZDOTDIR := environmentValue(env, "ZDOTDIR")
		if originalZDOTDIR == "" {
			originalZDOTDIR = environmentValue(env, "HOME")
		}
		env = setEnvironmentValue(env, shellIntegrationNonceEnvironment, nonce)
		env = setEnvironmentValue(env, shellIntegrationWrapperEnvironment, owner.directory)
		env = setEnvironmentValue(env, shellIntegrationOriginalZDOTDIR, originalZDOTDIR)
		env = setEnvironmentValue(env, "ZDOTDIR", owner.directory)
		return args, env, ShellIntegrationDescriptor{Quality: ShellIntegrationRich, Nonce: nonce}
	case "bash":
		login, ok := supportedBashArgs(args)
		if !ok || login {
			return args, env, ShellIntegrationDescriptor{Quality: ShellIntegrationNone}
		}
		args = []string{"--init-file", owner.bashRegular, "-i"}
		env = setEnvironmentValue(env, shellIntegrationNonceEnvironment, nonce)
		return args, env, ShellIntegrationDescriptor{Quality: ShellIntegrationRich, Nonce: nonce}
	default:
		return args, env, ShellIntegrationDescriptor{Quality: ShellIntegrationNone}
	}
}

func (owner *shellIntegrationOwner) supports(profile Profile) bool {
	return owner != nil && supportsShellIntegration(profile)
}

func supportsShellIntegration(profile Profile) bool {
	if profile.Kind != ProfileShell {
		return false
	}
	switch strings.ToLower(filepath.Base(profile.Executable)) {
	case "zsh":
		return supportedInteractiveShellArgs(profile.Args)
	case "bash":
		login, ok := supportedBashArgs(profile.Args)
		return ok && !login
	default:
		return false
	}
}

func supportedInteractiveShellArgs(args []string) bool {
	for _, argument := range args {
		if argument == "--login" || argument == "--interactive" {
			continue
		}
		if !strings.HasPrefix(argument, "-") || argument == "-" || argument == "--" {
			return false
		}
		for _, flag := range strings.TrimPrefix(argument, "-") {
			if flag != 'l' && flag != 'i' {
				return false
			}
		}
	}
	return true
}

func supportedBashArgs(args []string) (bool, bool) {
	if !supportedInteractiveShellArgs(args) {
		return false, false
	}
	login := false
	for _, argument := range args {
		login = login || argument == "--login" || strings.Contains(strings.TrimPrefix(argument, "-"), "l")
	}
	return login, true
}

func environmentValue(environment []string, key string) string {
	for _, entry := range environment {
		name, value, ok := strings.Cut(entry, "=")
		if ok && name == key {
			return value
		}
	}
	return ""
}

func setEnvironmentValue(environment []string, key, value string) []string {
	result := make([]string, 0, len(environment)+1)
	replaced := false
	for _, entry := range environment {
		name, _, ok := strings.Cut(entry, "=")
		if ok && name == key {
			if !replaced {
				result = append(result, key+"="+value)
				replaced = true
			}
			continue
		}
		result = append(result, entry)
	}
	if !replaced {
		result = append(result, key+"="+value)
	}
	return result
}

func (owner *shellIntegrationOwner) Close() error {
	if owner == nil || owner.directory == "" {
		return nil
	}
	return os.RemoveAll(owner.directory)
}

func zshStartupSource(name string) string {
	return `if [[ -n "${` + shellIntegrationOriginalZDOTDIR + `:-}" && "${` +
		shellIntegrationOriginalZDOTDIR + `}" != "${` + shellIntegrationWrapperEnvironment + `}" && -r "${` +
		shellIntegrationOriginalZDOTDIR + `}/` + name + `" ]]; then
  source "${` + shellIntegrationOriginalZDOTDIR + `}/` + name + `"
fi
`
}

const zshEnvironmentIntegration = `if [[ -n "${` + shellIntegrationOriginalZDOTDIR + `:-}" && "${` + shellIntegrationOriginalZDOTDIR + `}" != "${` + shellIntegrationWrapperEnvironment + `}" && -r "${` + shellIntegrationOriginalZDOTDIR + `}/.zshenv" ]]; then
  source "${` + shellIntegrationOriginalZDOTDIR + `}/.zshenv"
fi
export ZDOTDIR="${` + shellIntegrationWrapperEnvironment + `}"
`

const zshRestoreWrapperDirectory = `export ZDOTDIR="${` + shellIntegrationWrapperEnvironment + `}"
`

const zshHookIntegration = `
typeset -g __ptrack_shell_nonce="${` + shellIntegrationNonceEnvironment + `:-}"
unset ` + shellIntegrationNonceEnvironment + `
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
export ZDOTDIR="${` + shellIntegrationOriginalZDOTDIR + `}"
`

const bashRegularIntegration = `if [[ -r ~/.bashrc ]]; then source ~/.bashrc; fi
` + bashHookIntegration

const bashHookIntegration = `
__ptrack_shell_nonce="${` + shellIntegrationNonceEnvironment + `:-}"
unset ` + shellIntegrationNonceEnvironment + `
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
`
