use failure::Error;
use rustwide::cmd::SandboxBuilder;
use rustwide::{Crate, Toolchain};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const WORKSPACE_NAME: &str = "purge-caches";

#[test]
fn test_purge_caches() -> Result<(), Error> {
    let workspace_path = crate::utils::workspace_path(WORKSPACE_NAME);
    let workspace = crate::utils::init_named_workspace(WORKSPACE_NAME)?;

    // Do an initial purge to prevent stale files from being present.
    workspace.purge_all_build_dirs()?;
    workspace.purge_all_caches()?;

    let toolchain = Toolchain::dist("stable");
    toolchain.install(&workspace)?;

    let start_contents = WorkspaceContents::collect(&workspace_path)?;

    let crates = vec![
        Crate::crates_io("lazy_static", "1.0.0"),
        Crate::git("https://github.com/pietroalbini/git-credential-null"),
    ];

    // Simulate a build, which is going to fill up the caches.
    for krate in &crates {
        krate.fetch(&workspace)?;

        let sandbox = SandboxBuilder::new().enable_networking(false);
        let mut build_dir = workspace.build_dir("shared");
        build_dir.build(&toolchain, krate, sandbox).run(|build| {
            build.cargo().args(&["check"]).run()?;
            Ok(())
        })?;
    }

    // After all the builds are done purge everything again, and ensure the contents are the same
    // as when we started.
    workspace.purge_all_build_dirs()?;
    workspace.purge_all_caches()?;
    let end_contents = WorkspaceContents::collect(&workspace_path)?;
    start_contents.assert_same(end_contents);

    Ok(())
}

/// Define which files should be ignored when comparing the two workspaces. If there are expected
/// changes, update the function to match them.
fn should_ignore(base: &Path, path: &Path) -> bool {
    let components = match path.strip_prefix(base) {
        Ok(stripped) => stripped
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        Err(_) => return false,
    };

    let components = components.iter().map(|c| c.as_str()).collect::<Vec<_>>();
    match components.as_slice() {
        // The indexes could be updated during the build. The index is not considered a cache
        // though, so it's fine to ignore it during the comparison.
        ["cargo-home", "registry", "index", _, ".git", ..] => true,
        ["cargo-home", "registry", "index", _, ".cargo-index-lock"] => true,
        ["cargo-home", "registry", "index", _, ".last-updated"] => true,

        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct WorkspaceContents {
    base: PathBuf,
    files: HashMap<PathBuf, Digest>,
}

impl WorkspaceContents {
    fn collect(path: &Path) -> Result<Self, Error> {
        let mut files = HashMap::new();

        for entry in walkdir::WalkDir::new(path) {
            let entry = entry?;
            if !entry.metadata()?.is_file() {
                continue;
            }

            let mut sha = Sha1::new();
            sha.update(&std::fs::read(entry.path())?);

            files.insert(entry.path().into(), sha.digest());
        }

        Ok(Self {
            base: path.into(),
            files,
        })
    }

    fn assert_same(self, mut other: Self) {
        let mut same = true;

        println!("=== start directory differences ===");

        for (path, start_digest) in self.files.into_iter() {
            if should_ignore(&self.base, &path) {
                continue;
            }

            if let Some(end_digest) = other.files.remove(&path) {
                if start_digest != end_digest {
                    println!("file {} changed", path.display());
                    same = false;
                }
            } else {
                println!("file {} was removed", path.display());
                same = false;
            }
        }

        for (path, _) in other.files.into_iter() {
            if should_ignore(&other.base, &path) {
                continue;
            }

            println!("file {} was added", path.display());
            same = false;
        }

        println!("=== end directory differences ===");

        if !same {
            panic!("the contents of the directory changed");
        }
    }
}
