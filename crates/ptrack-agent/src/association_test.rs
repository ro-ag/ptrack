use std::collections::BTreeMap;
use std::fs;

use super::{
    ASSOCIATION_VERSION_V1, Association, AssociationCatalog, AssociationError, AssociationHost,
    AssociationPointer, AssociationTarget, association_generation, association_project_root,
    bind_association,
};
use crate::test_support::TempDirectory;

struct Catalog {
    plans: Vec<u64>,
    tasks: BTreeMap<u64, u64>,
}

impl AssociationCatalog for Catalog {
    fn validate_plan(&self, plan_id: u64) -> Result<(), String> {
        self.plans
            .contains(&plan_id)
            .then_some(())
            .ok_or_else(|| "not found".to_owned())
    }

    fn task_plan(&self, task_id: u64) -> Result<u64, String> {
        self.tasks
            .get(&task_id)
            .copied()
            .ok_or_else(|| "not found".to_owned())
    }
}

#[test]
fn host_validates_and_mints_monotonic_authority_free_associations() {
    let root = TempDirectory::new("ptrack-agent-association-root");
    let links = TempDirectory::new("ptrack-agent-association-link");
    let alias = links.path().join("project");
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path(), &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(root.path(), &alias).unwrap();
    let catalog = Catalog {
        plans: vec![2],
        tasks: BTreeMap::from([(9, 2)]),
    };
    let host = AssociationHost::new(&alias, 7, Some(&catalog)).unwrap();
    let first = host
        .bind(
            "opaque-live-id",
            AssociationPointer {
                version: 1,
                plan_id: 2,
                task_id: 9,
            },
            None,
        )
        .unwrap();
    assert_eq!(
        first.project_root,
        fs::canonicalize(root.path()).unwrap().to_string_lossy()
    );
    assert_eq!(first.revision, 1);
    let second = host
        .bind(
            "opaque-live-id",
            AssociationPointer {
                version: 1,
                plan_id: 2,
                task_id: 0,
            },
            Some(&first),
        )
        .unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(
        second.target,
        AssociationTarget {
            plan_id: 2,
            task_id: 0
        }
    );
}

#[test]
fn host_rejects_unsupported_invalid_and_stale_associations() {
    let root = TempDirectory::new("ptrack-agent-association-errors");
    let catalog = Catalog {
        plans: vec![1, 2],
        tasks: BTreeMap::from([(8, 2)]),
    };
    let host = AssociationHost::new(root.path(), 3, Some(&catalog)).unwrap();
    let cases = [
        (
            AssociationPointer {
                version: 2,
                ..AssociationPointer::default()
            },
            "unsupported association version: 2",
        ),
        (
            AssociationPointer {
                version: 1,
                task_id: 8,
                ..AssociationPointer::default()
            },
            "invalid association target: task requires a plan",
        ),
        (
            AssociationPointer {
                version: 1,
                plan_id: 99,
                task_id: 0,
            },
            "invalid association target: plan #99: not found",
        ),
        (
            AssociationPointer {
                version: 1,
                plan_id: 2,
                task_id: 99,
            },
            "invalid association target: task #99: not found",
        ),
        (
            AssociationPointer {
                version: 1,
                plan_id: 1,
                task_id: 8,
            },
            "invalid association target: task #8 belongs to plan #2, not plan #1",
        ),
    ];
    for (pointer, expected) in cases {
        assert_eq!(
            host.bind("live", pointer, None).unwrap_err().to_string(),
            expected
        );
    }
    let mut previous = host
        .bind(
            "live",
            AssociationPointer {
                version: 1,
                ..AssociationPointer::default()
            },
            None,
        )
        .unwrap();
    previous.generation = 2;
    assert_eq!(
        host.bind(
            "live",
            AssociationPointer {
                version: 1,
                ..AssociationPointer::default()
            },
            Some(&previous)
        )
        .unwrap_err(),
        AssociationError::Stale(None)
    );
    assert_eq!(
        AssociationHost::new(root.path(), 0, None)
            .err()
            .unwrap()
            .to_string(),
        "stale association: workspace generation must be nonzero"
    );
}

#[test]
fn association_json_contains_only_live_context_metadata() {
    let encoded = serde_json::to_string(&Association {
        version: ASSOCIATION_VERSION_V1,
        project_root: "/project".to_owned(),
        generation: 3,
        live_id: "opaque-live-id".to_owned(),
        target: AssociationTarget {
            plan_id: 2,
            task_id: 9,
        },
        revision: 4,
    })
    .unwrap();
    assert_eq!(
        encoded,
        r#"{"version":1,"projectRoot":"/project","generation":3,"liveId":"opaque-live-id","target":{"planId":2,"taskId":9},"revision":4}"#
    );
    assert_eq!(
        serde_json::to_string(&AssociationPointer {
            version: 1,
            plan_id: 0,
            task_id: 0
        })
        .unwrap(),
        r#"{"version":1}"#
    );
}

#[test]
fn optional_host_entrypoints_fail_closed_and_getters_are_empty() {
    assert_eq!(association_project_root(None), std::path::Path::new(""));
    assert_eq!(association_generation(None), 0);
    assert_eq!(
        bind_association(
            None,
            "live",
            AssociationPointer {
                version: 1,
                ..AssociationPointer::default()
            },
            None,
        )
        .unwrap_err(),
        AssociationError::HostRequired
    );
}
