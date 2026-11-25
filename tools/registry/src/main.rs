//! `devspace-registry`, the maintainer tool of the DevSpace package index.
//!
//! The package index (`registry/index.json`) lists the runtime archives that
//! DevSpace may install, with their SHA-256 checksums. It is signed with
//! the maintainer's Ed25519 key in the minisign format, and DevSpace only
//! uses an index whose signature matches the public key embedded in it
//! (`registry/devspace.pub`).
//!
//! - `keygen` creates a key pair; the secret key is encrypted with a
//!   password asked interactively and must stay off the repository;
//! - `sign` signs a file (`<file>.minisig`), asking for that password;
//! - `verify` checks a signature.
//!
//! The files are compatible with the reference `minisign` tool.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use devspace_platform::fs::FileMode;
use minisign::{KeyPair, PublicKey, PublicKeyBox, SecretKey, SignatureBox};

#[derive(Debug, Parser)]
#[command(
    name = "devspace-registry",
    version,
    about = "Sign and verify the DevSpace package index"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a key pair; the secret key is encrypted with a password.
    /// Existing files are never overwritten.
    Keygen {
        /// Secret key to create (keep it outside the repository).
        #[arg(long)]
        secret_key: PathBuf,
        /// Public key to create.
        #[arg(long)]
        public_key: PathBuf,
    },
    /// Sign a file; the signature is written to `<file>.minisig`.
    Sign {
        /// Secret key created by `keygen`.
        #[arg(long)]
        secret_key: PathBuf,
        /// Public key of the pair, checked against the signature.
        #[arg(long)]
        public_key: PathBuf,
        /// File to sign.
        file: PathBuf,
    },
    /// Verify `<file>.minisig`.
    Verify {
        /// Public key.
        #[arg(long)]
        public_key: PathBuf,
        /// Signed file.
        file: PathBuf,
    },
}

/// Failure of a command.
#[derive(Debug)]
enum ToolError {
    Io(PathBuf, io::Error),
    Minisign(minisign::PError),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, err) => write!(f, "{}: {err}", path.display()),
            Self::Minisign(err) => write!(f, "{err}"),
        }
    }
}

impl From<minisign::PError> for ToolError {
    fn from(err: minisign::PError) -> Self {
        Self::Minisign(err)
    }
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<(), ToolError> {
    match command {
        Command::Keygen {
            secret_key,
            public_key,
        } => {
            // Both files are created before the key exists, so that a
            // refusal to overwrite happens before the password prompt.
            let mut sk_file = create_new(&secret_key, true)?;
            let mut pk_file = match create_new(&public_key, false) {
                Ok(file) => file,
                Err(err) => {
                    let _ = fs::remove_file(&secret_key);
                    return Err(err);
                }
            };
            let result = (|| {
                let KeyPair { pk, sk } = KeyPair::generate_encrypted_keypair(None)?;
                let sk_box = sk.to_box(Some("DevSpace package index secret key"))?;
                let pk_box = pk.to_box()?;
                write_all(&mut sk_file, &secret_key, &sk_box.to_bytes())?;
                write_all(&mut pk_file, &public_key, &pk_box.to_bytes())?;
                println!("public key: {}", pk.to_base64());
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&secret_key);
                let _ = fs::remove_file(&public_key);
            }
            result
        }
        Command::Sign {
            secret_key,
            public_key,
            file,
        } => {
            let pk = load_public_key(&public_key)?;
            let sk = SecretKey::from_file(&secret_key, None)?;
            let data = File::open(&file).map_err(|err| ToolError::Io(file.clone(), err))?;
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let signature = minisign::sign(
                Some(&pk),
                &sk,
                data,
                Some(&format!("devspace package index {name}")),
                Some("signature from the DevSpace package index key"),
            )?;
            let out = signature_path(&file);
            fs::write(&out, signature.to_bytes()).map_err(|err| ToolError::Io(out.clone(), err))?;
            println!("signed {} → {}", file.display(), out.display());
            Ok(())
        }
        Command::Verify { public_key, file } => {
            let pk = load_public_key(&public_key)?;
            let sig_path = signature_path(&file);
            let signature = SignatureBox::from_file(&sig_path)?;
            let data = File::open(&file).map_err(|err| ToolError::Io(file.clone(), err))?;
            minisign::verify(&pk, &signature, data, true, false, false)?;
            println!("{}: valid signature", file.display());
            Ok(())
        }
    }
}

fn load_public_key(path: &Path) -> Result<PublicKey, ToolError> {
    let text = fs::read_to_string(path).map_err(|err| ToolError::Io(path.to_path_buf(), err))?;
    Ok(PublicKeyBox::from_string(&text)?.into_public_key()?)
}

fn signature_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".minisig");
    PathBuf::from(name)
}

fn create_new(path: &Path, private: bool) -> Result<File, ToolError> {
    let mode = if private {
        FileMode::Private
    } else {
        FileMode::Readable
    };
    devspace_platform::fs::create_new(path, mode)
        .map_err(|err| ToolError::Io(path.to_path_buf(), err))
}

fn write_all(file: &mut File, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|err| ToolError::Io(path.to_path_buf(), err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn signature_sits_next_to_the_file() {
        assert_eq!(
            signature_path(Path::new("registry/index.json")),
            PathBuf::from("registry/index.json.minisig")
        );
    }
}
