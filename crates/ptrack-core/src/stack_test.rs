use crate::model::LanguageId;
use crate::stack::{MAX_STACK_PROJECTS, resolve};

fn paths(values: &[&str]) -> Vec<String> {
    let mut owned: Vec<String> = values.iter().map(|value| (*value).to_owned()).collect();
    owned.sort();
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
    let mut values: Vec<String> = (0..MAX_STACK_PROJECTS + 10)
        .map(|index| format!("crates/c{index:03}/Cargo.toml"))
        .collect();
    values.push("Cargo.toml".to_owned());
    values.sort();
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
