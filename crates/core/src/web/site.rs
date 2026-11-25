//! What the local site of each project serves.
//!
//! The target is read from the project manifest every time the web server
//! configuration is generated, so that editing `devspace.toml` only needs a
//! restart of the web server:
//!
//! 1. `[web] proxy = "<loopback address>"`: requests are forwarded to that
//!    development server;
//! 2. `[web] root = "<dir>"`: files of that directory;
//! 3. otherwise the `public` directory of the project, if it exists.
//!
//! A root holding an `index.php` and no `index.html` is a PHP site, served
//! through the PHP-FPM of the version chosen by `[runtimes] php` (the
//! newest installed version when absent); other roots are static. A root
//! must resolve (symbolic links included) to a directory inside the
//! project. A project whose target cannot be determined is
//! reported as [`SiteTarget::Unavailable`] and not served; this never
//! prevents the other sites from being served.

use std::path::{Path, PathBuf};

use devspace_helper::Tld;
use devspace_platform::paths::OsFamily;

use crate::error::report;
use crate::project::manifest::{Manifest, ProxyUpstream};
use crate::project::{Project, ProjectName};
use crate::runtime::{
    self, InstalledRuntime, RuntimeLayout, RuntimeName, Selected, Selection, Version,
};

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
    /// PHP application: files of a directory inside the project, `.php`
    /// scripts executed by PHP-FPM.
    Php {
        /// Document root (canonical path).
        root: PathBuf,
        /// PHP version.
        version: Version,
        /// FastCGI socket of the PHP-FPM of that version.
        socket: PathBuf,
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

/// Installed runtimes, to serve PHP sites.
#[derive(Debug, Clone, Copy)]
pub struct Runtimes<'a> {
    /// Installed versions.
    pub installed: &'a [InstalledRuntime],
    /// Their layout (PHP-FPM sockets).
    pub layout: &'a RuntimeLayout,
}

/// Determines the site of `project` under `tld` from its manifest and
/// directory.
#[must_use]
pub fn resolve(project: &Project, tld: &Tld, runtimes: Runtimes<'_>) -> Site {
    Site {
        project: project.name.clone(),
        domain: domain(&project.name, tld),
        target: target(&project.path, runtimes),
    }
}

fn target(project_dir: &Path, runtimes: Runtimes<'_>) -> SiteTarget {
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
    let selection = manifest
        .runtimes
        .php
        .map_or(Selection::Newest, Selection::Manifest);
    let (relative, explicit) = match &manifest.web.root {
        Some(root) => (root.as_path(), true),
        None => (Path::new(DEFAULT_WEB_ROOT), false),
    };
    match web_root(project_dir, relative, explicit) {
        Ok(root) if is_php_root(&root) => php(root, selection, runtimes),
        Ok(root) => SiteTarget::Static { root },
        Err(reason) => SiteTarget::Unavailable { reason },
    }
}

/// Whether a root is a PHP front controller rather than static files.
fn is_php_root(root: &Path) -> bool {
    let has_static_index = ["index.html", "index.htm"]
        .iter()
        .any(|index| root.join(index).is_file());
    !has_static_index && root.join("index.php").is_file()
}

fn php(root: PathBuf, selection: Selection, runtimes: Runtimes<'_>) -> SiteTarget {
    let unavailable = |reason: String| SiteTarget::Unavailable { reason };
    if OsFamily::current() == OsFamily::Windows {
        return unavailable(
            "PHP sites are not served on Windows yet; run a development server (e.g. `php \
             artisan serve --port 8000`) and set `[web] proxy = \"127.0.0.1:8000\"` in \
             devspace.toml"
                .to_owned(),
        );
    }
    match runtime::select(runtimes.installed, RuntimeName::Php, selection) {
        Selected::Installed(php) => SiteTarget::Php {
            socket: runtimes.layout.fpm(php.version).socket,
            root,
            version: php.version,
        },
        Selected::Missing => match selection {
            Selection::Manifest(req) => unavailable(format!(
                "PHP {req} (required by devspace.toml) is not installed; run `devspace runtime \
                 install php {req}`"
            )),
            Selection::Newest => unavailable(
                "PHP project (index.php) but no PHP version is installed; run `devspace runtime \
                 install php`"
                    .to_owned(),
            ),
        },
    }
}

/// Resolves a web root; `explicit` tells whether the manifest chose it.
fn web_root(project_dir: &Path, relative: &Path, explicit: bool) -> Result<PathBuf, String> {
    let candidate = project_dir.join(relative);
    let canonical = match devspace_platform::fs::canonicalize(&candidate) {
        Ok(canonical) => canonical,
        Err(_) if !explicit => {
            return Err(format!(
                "no `{DEFAULT_WEB_ROOT}` directory; set `[web] root` or `[web] proxy` in \
                 devspace.toml"
            ));
        }
        Err(err) => {
            return Err(format!(
                "web.root `{}` cannot be accessed: {err}",
                relative.display()
            ));
        }
    };
    if !canonical.is_dir() {
        return Err(format!("`{}` is not a directory", candidate.display()));
    }
    // The project path is canonical, so a prefix check is enough to detect
    // symbolic links leaving the project.
    if !canonical.starts_with(project_dir) {
        return Err(format!(
            "`{}` resolves to `{}`, outside the project",
            candidate.display(),
            canonical.display()
        ));
    }
    if canonical.to_str().is_none() {
        return Err(format!("`{}` is not valid UTF-8", canonical.display()));
    }
    Ok(canonical)
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

    fn resolve_with(project: &Project, installed: &[InstalledRuntime]) -> Site {
        let layout = RuntimeLayout::new(&crate::AppPaths::from_root(Path::new("/devspace")));
        resolve(
            project,
            &Tld::default(),
            Runtimes {
                installed,
                layout: &layout,
            },
        )
    }

    fn php_runtime(version: &str) -> InstalledRuntime {
        InstalledRuntime {
            name: RuntimeName::Php,
            version: Version::parse(version).unwrap(),
            dir: PathBuf::from("/devspace/data/runtimes/php").join(version),
            bin_dirs: Vec::new(),
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
        let site = resolve_with(&project, &[]);
        assert_eq!(site.domain, "blog.internal");
        assert!(reason(&site).contains("no `public` directory"));

        fs::create_dir(tmp.path().join("public")).unwrap();
        assert_eq!(
            resolve_with(&project, &[]).target,
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
            reason(&resolve_with(&project, &[])).contains("web.root `dist` cannot be accessed")
        );
        fs::create_dir(tmp.path().join("dist")).unwrap();
        assert_eq!(
            resolve_with(&project, &[]).target,
            SiteTarget::Static {
                root: project.path.join("dist")
            }
        );

        fs::write(&manifest, "[web]\nproxy = \"127.0.0.1:5173\"\n").unwrap();
        assert_eq!(
            resolve_with(&project, &[]).target,
            SiteTarget::Proxy {
                upstream: ProxyUpstream::parse("127.0.0.1:5173").unwrap()
            }
        );

        fs::write(&manifest, "[web]\nroot = 3\n").unwrap();
        assert!(reason(&resolve_with(&project, &[])).contains("invalid project manifest"));
    }

    #[test]
    fn php_projects_use_the_selected_php() {
        let tmp = tempfile::tempdir().unwrap();
        let public = tmp.path().join("public");
        fs::create_dir(&public).unwrap();
        fs::write(public.join("index.php"), "<?php").unwrap();
        let project = project(tmp.path());
        let installed = [php_runtime("8.4.23"), php_runtime("8.5.8")];

        if OsFamily::current() == OsFamily::Windows {
            assert!(reason(&resolve_with(&project, &installed)).contains("not served on Windows"));
            return;
        }
        assert!(reason(&resolve_with(&project, &[])).contains("devspace runtime install php"));
        assert_eq!(
            resolve_with(&project, &installed).target,
            SiteTarget::Php {
                root: project.path.join("public"),
                version: Version::parse("8.5.8").unwrap(),
                socket: PathBuf::from("/devspace/run/php/8.5.8.sock"),
            },
            "newest installed version by default"
        );

        let manifest = tmp.path().join(MANIFEST_FILE_NAME);
        fs::write(&manifest, "[runtimes]\nphp = \"8.4\"\n").unwrap();
        assert!(matches!(
            resolve_with(&project, &installed).target,
            SiteTarget::Php { version, .. } if version.to_string() == "8.4.23"
        ));
        fs::write(&manifest, "[runtimes]\nphp = \"8.3\"\n").unwrap();
        let site = resolve_with(&project, &installed);
        assert!(
            reason(&site).contains("PHP 8.3 (required by devspace.toml) is not installed"),
            "{site:?}"
        );

        fs::write(public.join("index.html"), "<h1>static</h1>").unwrap();
        assert!(matches!(
            resolve_with(&project, &installed).target,
            SiteTarget::Static { .. }
        ));
    }

    #[test]
    fn root_must_be_a_directory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("public"), "not a directory").unwrap();
        assert!(reason(&resolve_with(&project(tmp.path()), &[])).contains("is not a directory"));
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
        assert!(reason(&resolve_with(&project(&project_dir), &[])).contains("outside the project"));
    }
}
