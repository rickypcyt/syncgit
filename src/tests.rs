use super::*;
use std::fs;
use std::path::Path;

#[test]
fn parses_https_and_ssh_repository_names() {
    assert_eq!(
        GitRepo::parse_repo_name_from_url("https://github.com/user/project.git"),
        Some("project".to_string())
    );
    assert_eq!(
        GitRepo::parse_repo_name_from_url("git@github.com:user/project.git"),
        Some("project".to_string())
    );
    assert_eq!(
        GitRepo::parse_repo_name_from_url("https://github.com/user/"),
        None
    );
}

#[test]
fn computes_root_and_nested_pathspecs() {
    let root = Path::new("C:/workspace/repo");
    assert_eq!(compute_pathspec(root, root), ".");
    assert_eq!(
        compute_pathspec(root, Path::new("C:/workspace/repo/src")),
        "src"
    );
}

#[test]
fn normalizes_pathspec_separators_and_newlines() {
    assert_eq!(
        GitRepo::normalize_pathspec("src\\nested\nfile"),
        "src/nestedfile"
    );
}

#[test]
fn finds_repository_from_nested_directory() {
    let root = std::env::temp_dir().join(format!("syncgit-test-{}", std::process::id()));
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir(root.join(".git")).unwrap();

    let repo = GitRepo::find_from_path(&nested).unwrap();
    assert_eq!(repo.root, root);

    fs::remove_dir_all(&root).unwrap();
}
