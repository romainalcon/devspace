//! What the local site of each project serves.
//!
//! The target is read from the project manifest every time the web server
//! configuration is generated, so that editing `devspace.toml` only needs a
//! restart of the web server:
//!
//! 1. `[web] proxy = "<loopback address>"`: requests are forwarded to that
//!    development server;
//! 2. `[web] root = "<dir>"`: static files of that directory;
//! 3. otherwise the `public` directory of the project, if it exists.
//!
//! A static root must resolve (symbolic links included) to a directory
//! inside the project. A project whose target cannot be determined is
//! reported as [`SiteTarget::Unavailable`] and not served; this never
//! prevents the other sites from being served.

use std::path::{Path, PathBuf};

use devspace_helper::Tld;

use crate::error::report;
use crate::project::manifest::{Manifest, ProxyUpstream};
use crate::project::{Project, ProjectName};

/// Directory served when the manifest does not choose a target.
pub const DEFAULT_WEB_ROOT: &str = "public";

/// Local site of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// Project name.
    pub project: ProjectName,
    /// Domain of the site, `<name>.<tld>`; subdomains are served too.
    pub domain: String,
    /// What the site serves.
    pub target: SiteTarget,
}

/// Content of a site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteTarget {
    /// Static files of a directory inside the project (canonical path).
    Static {
        /// Directory served.
        root: PathBuf,
    },
    /// Development server on the loopback interface.
    Proxy {
        /// Address of the server.
        upstream: ProxyUpstream,
    },
    /// Not served.
    Unavailable {
        /// Explanation for the user.
        reason: String,
    },
}

/// Domain of the site of `name`.
#[must_use]
pub fn domain(name: &ProjectName, tld: &Tld) -> String {
    format!("{name}.{tld}")
}

/// Determines the site of `project` under `tld` from its manifest and
/// directory.
#[must_use]
pub fn resolve(project: &Project, tld: &Tld) -> Site {
    Site {
        project: project.name.clone(),
        domain: domain(&project.name, tld),
        target: target(&project.path),
    }
}

fn target(project_dir: &Path) -> SiteTarget {
    let manifest = match Manifest::load(project_dir) {
        Ok(manifest) => manifest.unwrap_or_default(),
        Err(err) => {
            return SiteTarget::Unavailable {
                reason: report(&err),
            };
        }
    };
    if let Some(upstream) = manifest.web.proxy {
        return SiteTarget::Proxy { upstream };
    }
    match manifest.web.root {
        Some(root) => static_root(project_dir, root.as_path(), true),
        None => static_root(project_dir, Path::new(DEFAULT_WEB_ROOT), false),
    }
}

/// Resolves a static root; `explicit` tells whether the manifest chose it.
fn static_root(project_dir: &Path, relative: &Path, explicit: bool) -> SiteTarget {
    let unavailable = |reason: String| SiteTarget::Unavailable { reason };
    let candidate = project_dir.join(relative);
    let canonical = match devspace_platform::fs::canonicalize(&candidate) {
        Ok(canonical) => canonical,
        Err(_) if !explicit => {
            return unavailable(format!(
                "no `{DEFAULT_WEB_ROOT}` directory; set `[web] root` or `[web] proxy` in \
                 devspace.toml"
            ));
        }
        Err(err) => {
            return unavailable(format!(
                "web.root `{}` cannot be accessed: {err}",
                relative.display()
            ));
        }
    };
    if !canonical.is_dir() {
        return unavailable(format!("`{}` is not a directory", candidate.display()));
    }
    // The project path is canonical, so a prefix check is enough to detect
    // symbolic links leaving the project.
    if !canonical.starts_with(project_dir) {
        return unavailable(format!(
            "`{}` resolves to `{}`, outside the project",
            candidate.display(),
            canonical.display()
        ));
    }
    if canonical.to_str().is_none() {
        return unavailable(format!("`{}` is not valid UTF-8", canonical.display()));
    }
    // A PHP front controller cannot be served as static files: the page
    // would be empty. PHP runtimes come with the package manager (M4).
    let has_static_index = ["index.html", "index.htm"]
        .iter()
        .any(|index| canonical.join(index).is_file());
    if !has_static_index && canonical.join("index.php").is_file() {
        return unavailable(format!(
            "PHP project (`{}` has an index.php): PHP runtimes are not available yet; \
             meanwhile run a development server (e.g. `php artisan serve --port 8000`) and set \
             `[web] proxy = \"127.0.0.1:8000\"` in devspace.toml",
            relative.display()
        ));
    }
    SiteTarget::Static { root: canonical }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::project::MANIFEST_FILE_NAME;

    fn project(dir: &Path) -> Project {
        Project {
            name: ProjectName::parse("blog").unwrap(),
            path: devspace_platform::fs::canonicalize(dir).unwrap(),
            added_at: 0,
        }
    }

    fn reason(site: &Site) -> &str {
        match &site.target {
            SiteTarget::Unavailable { reason } => reason,
            other => panic!("expected an unavailable site, got {other:?}"),
        }
    }

    #[test]
    fn public_directory_is_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let project = project(tmp.path());
        let site = resolve(&project, &Tld::default());
        assert_eq!(site.domain, "blog.internal");
        assert!(reason(&site).contains("no `public` directory"));

        fs::create_dir(tmp.path().join("public")).unwrap();
        assert_eq!(
            resolve(&project, &Tld::default()).target,
            SiteTarget::Static {
                root: project.path.join("public")
            }
        );
    }

    #[test]
    fn manifest_chooses_root_or_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let project = project(tmp.path());
        let manifest = tmp.path().join(MANIFEST_FILE_NAME);

        fs::write(&manifest, "[web]\nroot = \"dist\"\n").unwrap();
        assert!(
            reason(&resolve(&project, &Tld::default()))
                .contains("web.root `dist` cannot be accessed")
        );
        fs::create_dir(tmp.path().join("dist")).unwrap();
        assert_eq!(
            resolve(&project, &Tld::default()).target,
            SiteTarget::Static {
                root: project.path.join("dist")
            }
        );

        fs::write(&manifest, "[web]\nproxy = \"127.0.0.1:5173\"\n").unwrap();
        assert_eq!(
            resolve(&project, &Tld::default()).target,
            SiteTarget::Proxy {
                upstream: ProxyUpstream::parse("127.0.0.1:5173").unwrap()
            }
        );

        fs::write(&manifest, "[web]\nroot = 3\n").unwrap();
        assert!(reason(&resolve(&project, &Tld::default())).contains("invalid project manifest"));
    }

    #[test]
    fn php_projects_are_not_served_as_static_files() {
        let tmp = tempfile::tempdir().unwrap();
        let public = tmp.path().join("public");
        fs::create_dir(&public).unwrap();
        fs::write(public.join("index.php"), "<?php").unwrap();
        let project = project(tmp.path());
        assert!(reason(&resolve(&project, &Tld::default())).contains("PHP project"));

        fs::write(public.join("index.html"), "<h1>static</h1>").unwrap();
        assert!(matches!(
            resolve(&project, &Tld::default()).target,
            SiteTarget::Static { .. }
        ));
    }

    #[test]
    fn root_must_be_a_directory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("public"), "not a directory").unwrap();
        assert!(
            reason(&resolve(&project(tmp.path()), &Tld::default())).contains("is not a directory")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_leave_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("blog");
        let outside = tmp.path().join("secrets");
        fs::create_dir_all(&project_dir).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, project_dir.join("public")).unwrap();
        assert!(
            reason(&resolve(&project(&project_dir), &Tld::default()))
                .contains("outside the project")
        );
    }
}
