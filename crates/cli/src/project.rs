//! `devspace project add|list|remove`.
//!
//! The registry is owned by the daemon; these commands require it to run.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use devspace_core::AppPaths;
use devspace_ipc::{Endpoint, ProjectInfo, Request, Response};

use crate::client::{self, unexpected};
use crate::error::CliError;

/// Project subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ProjectCommand {
    /// Register a project directory (the current directory by default).
    ///
    /// The name is `--name` if given, otherwise `project.name` from the
    /// project's `devspace.toml`, otherwise derived from the directory name.
    /// The directory is not modified.
    Add {
        /// Project directory.
        path: Option<PathBuf>,
        /// Project name: lowercase letters, digits and hyphens.
        #[arg(long)]
        name: Option<String>,
    },
    /// List the registered projects.
    List,
    /// Unregister a project. Its directory is not modified.
    Remove {
        /// Name of the project.
        name: String,
    },
}

/// Runs a project subcommand.
pub fn execute(command: ProjectCommand, paths: &AppPaths) -> Result<ExitCode, CliError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    // Relative paths are resolved here: the daemon has its own working
    // directory.
    let request = match command {
        ProjectCommand::Add { path, name } => Request::ProjectAdd {
            path: std::path::absolute(path.as_deref().unwrap_or(Path::new(".")))
                .map_err(CliError::CurrentDir)?,
            name,
        },
        ProjectCommand::List => Request::ProjectList,
        ProjectCommand::Remove { name } => Request::ProjectRemove { name },
    };
    client::block_on(run(&endpoint, request))??;
    Ok(ExitCode::SUCCESS)
}

async fn run(endpoint: &Endpoint, request: Request) -> Result<(), CliError> {
    let mut client = client::connect(endpoint).await?;
    match client::request(&mut client, request).await? {
        Response::ProjectAdded(project) => {
            println!(
                "added project `{}` ({})",
                project.name,
                project.path.display()
            );
        }
        Response::ProjectRemoved(project) => {
            println!(
                "removed project `{}` ({}); its directory was not modified",
                project.name,
                project.path.display()
            );
        }
        Response::Projects { projects } => print!("{}", format_list(&projects)),
        _ => return Err(unexpected("project response")),
    }
    Ok(())
}

fn format_list(projects: &[ProjectInfo]) -> String {
    if projects.is_empty() {
        return "no projects registered; add one with `devspace project add [PATH]`\n".to_owned();
    }
    let width = projects
        .iter()
        .map(|p| p.name.len())
        .chain(["NAME".len()])
        .max()
        .unwrap_or_default();
    let mut out = format!("{:<width$}  PATH\n", "NAME");
    for project in projects {
        // Writing to a String cannot fail.
        let _ = writeln!(out, "{:<width$}  {}", project.name, project.path.display());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_aligned() {
        let project = |name: &str, path: &str| ProjectInfo {
            name: name.into(),
            path: PathBuf::from(path),
            added_at: 0,
        };
        assert_eq!(
            format_list(&[project("api", "/src/api"), project("my-blog", "/src/blog")]),
            "NAME     PATH\napi      /src/api\nmy-blog  /src/blog\n"
        );
        assert!(format_list(&[]).starts_with("no projects registered"));
    }
}
