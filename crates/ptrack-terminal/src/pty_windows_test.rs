use std::path::{Path, PathBuf};

use super::simplify_verbatim_cwd;

#[test]
fn verbatim_drive_path_is_stripped_to_a_plain_drive_path() {
    assert_eq!(
        simplify_verbatim_cwd(Path::new(r"\\?\C:\work\ptrack")),
        PathBuf::from(r"C:\work\ptrack")
    );
}

#[test]
fn verbatim_unc_path_is_rewritten_to_a_classic_unc_path() {
    assert_eq!(
        simplify_verbatim_cwd(Path::new(r"\\?\UNC\server\share\work")),
        PathBuf::from(r"\\server\share\work")
    );
}

#[test]
fn plain_drive_path_is_untouched() {
    assert_eq!(
        simplify_verbatim_cwd(Path::new(r"C:\work\ptrack")),
        PathBuf::from(r"C:\work\ptrack")
    );
}

#[test]
fn non_verbatim_unc_path_is_untouched() {
    assert_eq!(
        simplify_verbatim_cwd(Path::new(r"\\server\share\work")),
        PathBuf::from(r"\\server\share\work")
    );
}
