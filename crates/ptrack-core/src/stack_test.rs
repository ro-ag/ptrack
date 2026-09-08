use crate::model::LanguageId;
use crate::stack::{MAX_STACK_PROJECTS, TrackedFile, resolve};

fn paths(values: &[&str]) -> Vec<TrackedFile> {
    let mut owned: Vec<TrackedFile> = values
        .iter()
        .map(|value| TrackedFile::new(*value))
        .collect();
    owned.sort_by(|left, right| left.path.cmp(&right.path));
    owned
}

/// Tracked files carrying line counts, for the attribution tests.
fn counted(values: &[(&str, u32)]) -> Vec<TrackedFile> {
    let mut owned: Vec<TrackedFile> = values
        .iter()
        .map(|(path, lines)| TrackedFile {
            path: (*path).to_owned(),
            lines: *lines,
        })
        .collect();
    owned.sort_by(|left, right| left.path.cmp(&right.path));
    owned
}

#[test]
fn a_root_manifest_defines_one_project_with_its_evidence() {
    let resolved = resolve(&paths(&["Cargo.toml", "src/lib.rs", "src/main.rs"]));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].root, "");
    assert_eq!(resolved[0].language, LanguageId::Rust);
    assert_eq!(resolved[0].evidence, vec!["Cargo.toml".to_owned()]);
    assert_eq!(resolved[0].depth, 0);
    assert_eq!(resolved[0].files, 3);
}

#[test]
fn every_tracked_manifest_defines_its_own_project_ranked_shallowest_first() {
    let resolved = resolve(&paths(&[
        "Cargo.toml",
        "crates/one/Cargo.toml",
        "crates/one/src/lib.rs",
        "frontend/package.json",
        "frontend/src/app.js",
        "src/lib.rs",
    ]));
    let roots: Vec<&str> = resolved
        .iter()
        .map(|project| project.root.as_str())
        .collect();
    // Shallowest first: `frontend` sits one level down, `crates/one` two.
    assert_eq!(roots, vec!["", "frontend", "crates/one"]);
    assert_eq!(resolved[1].language, LanguageId::JavaScript);
    assert_eq!(resolved[1].files, 2);
    assert_eq!(resolved[2].files, 2);
}

#[test]
fn a_tracked_tsconfig_refines_javascript_to_typescript() {
    let resolved = resolve(&paths(&[
        "frontend/package.json",
        "frontend/tsconfig.json",
        "frontend/src/app.ts",
    ]));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].language, LanguageId::TypeScript);
    assert_eq!(
        resolved[0].evidence,
        vec![
            "frontend/package.json".to_owned(),
            "frontend/tsconfig.json".to_owned()
        ]
    );
}

#[test]
fn files_are_attributed_to_the_nearest_enclosing_project() {
    let resolved = resolve(&paths(&[
        "Cargo.toml",
        "crates/one/Cargo.toml",
        "crates/one/src/lib.rs",
        "crates/one/src/model.rs",
        "src/lib.rs",
    ]));
    assert_eq!(resolved[0].files, 2);
    assert_eq!(resolved[1].files, 3);
}

#[test]
fn resolution_is_independent_of_input_order() {
    let ordered = paths(&[
        "Cargo.toml",
        "crates/one/Cargo.toml",
        "crates/one/src/lib.rs",
        "frontend/package.json",
        "go.mod",
    ]);
    let expected = resolve(&ordered);
    let mut rotated = ordered.clone();
    rotated.rotate_left(2);
    let mut reversed = ordered;
    reversed.reverse();
    assert_eq!(resolve(&rotated), expected);
    assert_eq!(resolve(&reversed), expected);
}

#[test]
fn evidence_kept_under_the_cap_does_not_depend_on_input_order() {
    let ordered = paths(&[
        "tool/Cargo.toml",
        "tool/CMakeLists.txt",
        "tool/Dockerfile",
        "tool/Gemfile",
        "tool/Package.swift",
        "tool/build.gradle",
        "tool/composer.json",
        "tool/go.mod",
        "tool/mix.exs",
        "tool/package.json",
        "tool/pom.xml",
        "tool/pubspec.yaml",
    ]);
    let expected = resolve(&ordered);
    let mut reversed = ordered;
    reversed.reverse();
    assert_eq!(resolve(&reversed), expected);
    assert_eq!(expected[0].evidence.len(), 8);
    assert_eq!(expected[0].language, LanguageId::Rust);
}

#[test]
fn discovered_projects_are_capped_and_the_shallowest_survive() {
    let mut values: Vec<TrackedFile> = (0..MAX_STACK_PROJECTS + 10)
        .map(|index| TrackedFile::new(format!("crates/c{index:03}/Cargo.toml")))
        .collect();
    values.push(TrackedFile::new("Cargo.toml"));
    values.sort_by(|left, right| left.path.cmp(&right.path));
    let resolved = resolve(&values);
    assert_eq!(resolved.len(), MAX_STACK_PROJECTS);
    assert_eq!(resolved[0].root, "");
}

#[test]
fn a_tree_with_no_manifest_resolves_to_no_projects() {
    assert!(resolve(&paths(&["README.md", "docs/guide.md"])).is_empty());
}

#[test]
fn an_extension_marker_discovers_its_project() {
    let resolved = resolve(&paths(&["infra/main.tf", "infra/variables.tf"]));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].language, LanguageId::Terraform);
    assert_eq!(resolved[0].files, 2);
}

#[test]
fn tracked_typescript_sources_refine_a_project_with_no_tsconfig() {
    let resolved = resolve(&paths(&[
        "frontend/package.json",
        "frontend/src/app.ts",
        "frontend/vite.config.ts",
    ]));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].language, LanguageId::TypeScript);
    // No tsconfig.json is tracked, so it is never claimed as evidence.
    assert_eq!(
        resolved[0].evidence,
        vec!["frontend/package.json".to_owned()]
    );
}

#[test]
fn declaration_files_alone_leave_a_project_on_javascript() {
    let resolved = resolve(&paths(&[
        "frontend/package.json",
        "frontend/src/app.js",
        "frontend/types/global.d.ts",
    ]));
    assert_eq!(resolved[0].language, LanguageId::JavaScript);
}

#[test]
fn typescript_sources_refine_only_the_project_that_owns_them() {
    let resolved = resolve(&paths(&[
        "package.json",
        "src/index.js",
        "tools/package.json",
        "tools/main.ts",
    ]));
    let roots: Vec<(&str, LanguageId)> = resolved
        .iter()
        .map(|project| (project.root.as_str(), project.language))
        .collect();
    assert_eq!(
        roots,
        vec![
            ("", LanguageId::JavaScript),
            ("tools", LanguageId::TypeScript)
        ]
    );
}

#[test]
fn lines_are_attributed_to_the_nearest_enclosing_project() {
    let resolved = resolve(&counted(&[
        ("Cargo.toml", 12),
        ("src/lib.rs", 400),
        ("crates/one/Cargo.toml", 8),
        ("crates/one/src/lib.rs", 250),
    ]));
    assert_eq!(resolved[0].root, "");
    assert_eq!((resolved[0].files, resolved[0].lines), (2, 412));
    assert_eq!((resolved[1].files, resolved[1].lines), (2, 258));
}

#[test]
fn uncounted_files_contribute_no_lines_but_still_count_as_files() {
    let resolved = resolve(&counted(&[
        ("Cargo.toml", 12),
        ("assets/logo.png", 0),
        ("src/lib.rs", 100),
    ]));
    assert_eq!((resolved[0].files, resolved[0].lines), (3, 112));
}
