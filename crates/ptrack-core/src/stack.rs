//! Deterministic project discovery.
//!
//! The resolver is a pure function of a tracked-path list. It performs no
//! filesystem access, runs no subprocess, and reads no clock, so the same
//! listing always produces the same profile. Language identity comes from the
//! marker table below — never from how common an extension is in the tree, and
//! never from a size or line count.

use std::collections::BTreeMap;

use crate::model::{LanguageId, StackProfile, StackProject, StackSummary};

/// The most discovered projects one profile carries.
pub const MAX_STACK_PROJECTS: usize = 64;
/// The most evidence paths one discovered project carries.
pub const MAX_STACK_EVIDENCE: usize = 8;

/// How a marker matches a tracked path's final component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Marker {
    /// The exact file name.
    Name(&'static str),
    /// The file name's extension, without its dot.
    Extension(&'static str),
}

/// The fixed, ordered marker table. Order is part of the ranking contract:
/// when two markers sit in the same directory, the earlier entry wins.
const MARKERS: &[(Marker, LanguageId)] = &[
    (Marker::Name("Cargo.toml"), LanguageId::Rust),
    (Marker::Name("go.mod"), LanguageId::Go),
    (Marker::Name("package.json"), LanguageId::JavaScript),
    (Marker::Name("pyproject.toml"), LanguageId::Python),
    (Marker::Name("setup.py"), LanguageId::Python),
    (Marker::Name("Package.swift"), LanguageId::Swift),
    (Marker::Name("pom.xml"), LanguageId::Java),
    (Marker::Name("build.gradle"), LanguageId::Java),
    (Marker::Name("build.gradle.kts"), LanguageId::Kotlin),
    (Marker::Extension("csproj"), LanguageId::CSharp),
    (Marker::Name("Gemfile"), LanguageId::Ruby),
    (Marker::Name("composer.json"), LanguageId::Php),
    (Marker::Name("mix.exs"), LanguageId::Elixir),
    (Marker::Name("pubspec.yaml"), LanguageId::Dart),
    (Marker::Name("CMakeLists.txt"), LanguageId::C),
    (Marker::Extension("tf"), LanguageId::Terraform),
    (Marker::Name("Dockerfile"), LanguageId::Container),
];

/// The refinement marker: a tracked `tsconfig.json` beside a `package.json`
/// makes the project TypeScript rather than JavaScript.
const TYPESCRIPT_MARKER: &str = "tsconfig.json";

/// Resolves discovered projects from a tracked-path list.
///
/// Paths are repository-relative and use `/` separators. The result does not
/// depend on the order they arrive in.
#[must_use]
pub fn resolve(paths: &[String]) -> Vec<StackProject> {
    let mut discovered: BTreeMap<String, Discovery> = BTreeMap::new();
    let mut typescript_roots: Vec<&str> = Vec::new();

    for path in paths {
        let (directory, name) = split_path(path);
        if name == TYPESCRIPT_MARKER {
            typescript_roots.push(directory);
        }
        let Some((rank, language)) = match_marker(name) else {
            continue;
        };
        let entry = discovered
            .entry(directory.to_owned())
            .or_insert_with(|| Discovery {
                rank,
                language,
                evidence: Vec::new(),
            });
        if rank < entry.rank {
            entry.rank = rank;
            entry.language = language;
        }
        entry.evidence.push(path.clone());
    }

    let mut projects: Vec<StackProject> = discovered
        .into_iter()
        .map(|(root, discovery)| build_project(root, discovery, &typescript_roots))
        .collect();

    projects.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then_with(|| marker_rank(left.language).cmp(&marker_rank(right.language)))
            .then_with(|| left.root.cmp(&right.root))
    });
    projects.truncate(MAX_STACK_PROJECTS);

    attribute_files(&mut projects, paths);
    projects
}

/// Summarizes a profile for the global registry: language totals in profile
/// order, collapsing repeated languages into one entry.
#[must_use]
pub fn summarize(profile: &StackProfile) -> StackSummary {
    let mut languages: Vec<(LanguageId, u32)> = Vec::new();
    for project in &profile.projects {
        if let Some(entry) = languages
            .iter_mut()
            .find(|(language, _)| *language == project.language)
        {
            entry.1 = entry.1.saturating_add(project.files);
        } else {
            languages.push((project.language, project.files));
        }
    }
    StackSummary {
        languages,
        tracked_files: profile.tracked_files,
        scanned_head: profile.scanned_head.clone(),
        incomplete: profile.incomplete,
    }
}

/// One directory's accumulated marker evidence.
struct Discovery {
    rank: usize,
    language: LanguageId,
    evidence: Vec<String>,
}

fn build_project(root: String, discovery: Discovery, typescript_roots: &[&str]) -> StackProject {
    let Discovery {
        language,
        mut evidence,
        ..
    } = discovery;
    let language = if language == LanguageId::JavaScript
        && typescript_roots.iter().any(|value| *value == root)
    {
        LanguageId::TypeScript
    } else {
        language
    };
    if language == LanguageId::TypeScript {
        let marker = join_path(&root, TYPESCRIPT_MARKER);
        if !evidence.contains(&marker) {
            evidence.push(marker);
        }
    }
    // Sort before truncating: which evidence survives the cap must not depend
    // on the order the collector happened to report paths in.
    evidence.sort();
    evidence.truncate(MAX_STACK_EVIDENCE);
    let depth = u8::try_from(depth_of(&root)).unwrap_or(u8::MAX);
    StackProject {
        root,
        language,
        evidence,
        depth,
        files: 0,
    }
}

/// Attributes every tracked path to the deepest project whose root contains it.
fn attribute_files(projects: &mut [StackProject], paths: &[String]) {
    for path in paths {
        let mut best: Option<usize> = None;
        for (index, project) in projects.iter().enumerate() {
            if !contains(&project.root, path) {
                continue;
            }
            let deeper =
                best.is_none_or(|current| projects[current].root.len() < project.root.len());
            if deeper {
                best = Some(index);
            }
        }
        if let Some(index) = best {
            projects[index].files = projects[index].files.saturating_add(1);
        }
    }
}

fn contains(root: &str, path: &str) -> bool {
    if root.is_empty() {
        return true;
    }
    path.len() > root.len()
        && path.starts_with(root)
        && path.as_bytes().get(root.len()) == Some(&b'/')
}

fn match_marker(name: &str) -> Option<(usize, LanguageId)> {
    MARKERS
        .iter()
        .enumerate()
        .find(|(_, (marker, _))| match marker {
            Marker::Name(value) => *value == name,
            Marker::Extension(value) => name
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension == *value),
        })
        .map(|(index, (_, language))| (index, *language))
}

/// Returns the marker-table rank used to break depth ties. TypeScript is a
/// refinement of the JavaScript marker and shares its rank.
fn marker_rank(language: LanguageId) -> usize {
    let language = if language == LanguageId::TypeScript {
        LanguageId::JavaScript
    } else {
        language
    };
    MARKERS
        .iter()
        .position(|(_, value)| *value == language)
        .unwrap_or(MARKERS.len())
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/')
        .map_or(("", path), |(directory, name)| (directory, name))
}

fn join_path(root: &str, name: &str) -> String {
    if root.is_empty() {
        name.to_owned()
    } else {
        format!("{root}/{name}")
    }
}

fn depth_of(root: &str) -> usize {
    if root.is_empty() {
        0
    } else {
        root.matches('/').count() + 1
    }
}
