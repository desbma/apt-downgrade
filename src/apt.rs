use std::cmp::Ordering;
use std::collections::VecDeque;
use std::error;
use std::fmt;
use std::io::BufRead;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

use glob::glob;
use itertools::join;
use simple_error::SimpleError;

/// Package version with comparison traits
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PackageVersion {
    pub string: String,
}

impl Ord for PackageVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        deb_version::compare_versions(&self.string, &other.string)
    }
}

impl PartialOrd for PackageVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for PackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.string)
    }
}

/// A versioned package
#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub name: String,

    pub version: PackageVersion,
}

/// Dependency version relation
#[derive(Debug)]
pub enum PackageVersionRelation {
    Any,
    StrictlyInferior,
    InferiorOrEqual,
    Equal,
    SuperiorOrEqual,
    StriclySuperior,
}

/// Package dependency
#[derive(Debug)]
pub struct PackageDependency {
    pub package: Package,

    pub version_relation: PackageVersionRelation,
}

/// APT environement configuration values
struct AptEnv {
    arch: String,
    cache_dir: String,
}

lazy_static! {
    static ref APT_ENV: AptEnv = read_apt_env().expect("Unable to read APT environment");
}

/// Read APT environment values
fn read_apt_env() -> Result<AptEnv, Box<dyn error::Error>> {
    let output = Command::new("apt-config")
        .args(vec![
            "shell",
            "CACHE_ROOT_DIR",
            "Dir::Cache",
            "CACHE_ARCHIVE_SUBDIR",
            "Dir::Cache::archives",
            "ARCH",
            "APT::Architecture",
        ])
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(Box::new(SimpleError::new("apt-config failed")));
    }
    let lines: Vec<String> = output.stdout.lines().map(|l| l.unwrap()).collect();
    let cache_root_dir = lines
        .iter()
        .find(|l| l.starts_with("CACHE_ROOT_DIR="))
        .ok_or_else(|| SimpleError::new("Unexpected apt-config output"))?
        .split('\'')
        .nth(1)
        .ok_or_else(|| SimpleError::new("Unexpected apt-config output"))?;
    let archive_subdir = lines
        .iter()
        .find(|l| l.starts_with("CACHE_ARCHIVE_SUBDIR="))
        .ok_or_else(|| SimpleError::new("Unexpected apt-config output"))?
        .split('\'')
        .nth(1)
        .ok_or_else(|| SimpleError::new("Unexpected apt-config output"))?;
    let arch = lines
        .iter()
        .find(|l| l.starts_with("ARCH="))
        .ok_or_else(|| SimpleError::new("Unexpected apt-config output"))?
        .split('\'')
        .nth(1)
        .ok_or_else(|| SimpleError::new("Unexpected apt-config output"))?
        .to_string();

    let cache_dir = format!("/{}/{}", cache_root_dir, archive_subdir);

    Ok(AptEnv { cache_dir, arch })
}

/// Error generated when a command returns non zero code
#[derive(Debug)]
struct CommandError {
    status: std::process::ExitStatus,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status.code() {
            Some(code) => write!(f, "Command returned {}", code),
            None => write!(
                f,
                "Command killed by signal {}",
                self.status.signal().unwrap()
            ),
        }
    }
}

impl error::Error for CommandError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

/// Get dependencies for a package using local package cache
fn get_dependencies_cache(
    package: &Package,
) -> Result<VecDeque<PackageDependency>, Box<dyn error::Error>> {
    let mut deps = VecDeque::new();

    let deb_filepath = format!(
        "{}{}_{}_{}.deb",
        APT_ENV.cache_dir, package.name, package.version, APT_ENV.arch
    );
    let spec = format!("{}={}", package.name, package.version);
    let apt_args = if Path::new(&deb_filepath).is_file() {
        vec!["show", &deb_filepath]
    } else {
        vec!["show", &spec]
    };

    let output = Command::new("apt-cache")
        .args(apt_args)
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(Box::new(CommandError {
            status: output.status,
        }));
    }
    let line_prefix = "Depends: ";
    let package_desc_line = output
        .stdout
        .lines()
        .find(|l| l.as_ref().unwrap().starts_with(line_prefix))
        .ok_or_else(|| SimpleError::new("Unexpected apt-cache output"))??;
    for package_desc in package_desc_line
        .split_at(line_prefix.len())
        .1
        .split(',')
        .map(|l| l.trim_start())
    {
        let mut package_desc_tokens = package_desc.split(' ');
        let package_name = package_desc_tokens.next().unwrap().to_string();
        let package_version_relation_raw = &package_desc_tokens.next();
        let package_version_relation = match package_version_relation_raw {
            Some(r) => match &r[1..] {
                "<<" => PackageVersionRelation::StrictlyInferior,
                "<=" => PackageVersionRelation::InferiorOrEqual,
                "=" => PackageVersionRelation::Equal,
                ">=" => PackageVersionRelation::SuperiorOrEqual,
                ">>" => PackageVersionRelation::StriclySuperior,
                r => {
                    panic!("Unexpected version relation: {}", r);
                }
            },
            None => PackageVersionRelation::Any,
        };
        let package_version = match package_version_relation {
            PackageVersionRelation::Any => "",
            _ => {
                let package_version_raw = &package_desc_tokens.next().ok_or_else(|| SimpleError::new("Unexpected apt-cache output"))?;
                &package_version_raw[0..&package_version_raw.len() - 1]
            }
        };

        deps.push_back(PackageDependency {
            package: Package {
                name: package_name,
                version: PackageVersion {
                    string: package_version.to_string(),
                },
            },
            version_relation: package_version_relation,
        });
    }

    Ok(deps)
}

fn get_dependencies_remote(
    _package: &Package,
) -> Result<VecDeque<PackageDependency>, Box<dyn error::Error>> {
    // TODO Build download dir

    // TODO Check if already downloaded

    // TODO get http://ftp.debian.org/debian/pool/main/c/chromium/chromium_78.0.3904.108-1~deb10u1_amd64.deb

    // TODO get deps from deb

    unimplemented!();
}

/// Get dependencies for a package
pub fn get_dependencies(package: Package) -> VecDeque<PackageDependency> {
    match get_dependencies_cache(&package) {
        Ok(deps) => deps,
        Err(e) => {
            println!(
                "Failed to get dependencies for package {:?} from cache: {}",
                package, e
            );
            get_dependencies_remote(&package).unwrap()
        }
    }
}

/// Find the best package version that satisfies a dependency constraint
pub fn resolve_version(
    dependency: &PackageDependency,
    installed_version: &Option<PackageVersion>,
) -> Option<PackageVersion> {
    let version_candidates = get_cache_package_versions(dependency.package.name.clone());
    // TODO add remote versions

    match dependency.version_relation {
        PackageVersionRelation::Any => match installed_version {
            Some(v) => Some(v.clone()),
            None => version_candidates.first().cloned(),
        },
        PackageVersionRelation::StrictlyInferior => {
            if installed_version.is_some()
                && (*installed_version.as_ref().unwrap() < dependency.package.version)
            {
                installed_version.clone()
            } else {
                version_candidates
                    .iter()
                    .find(|v| **v < dependency.package.version)
                    .cloned()
            }
        }
        PackageVersionRelation::InferiorOrEqual => {
            if installed_version.is_some()
                && (*installed_version.as_ref().unwrap() <= dependency.package.version)
            {
                installed_version.clone()
            } else {
                version_candidates
                    .iter()
                    .find(|v| **v <= dependency.package.version)
                    .cloned()
            }
        }
        PackageVersionRelation::Equal => version_candidates
            .iter()
            .find(|v| v == &&dependency.package.version)
            .cloned(),
        PackageVersionRelation::SuperiorOrEqual => {
            if installed_version.is_some()
                && (*installed_version.as_ref().unwrap() >= dependency.package.version)
            {
                installed_version.clone()
            } else {
                version_candidates
                    .iter()
                    .find(|v| **v >= dependency.package.version)
                    .cloned()
            }
        }
        PackageVersionRelation::StriclySuperior => {
            if installed_version.is_some()
                && (*installed_version.as_ref().unwrap() > dependency.package.version)
            {
                installed_version.clone()
            } else {
                version_candidates
                    .iter()
                    .find(|v| **v > dependency.package.version)
                    .cloned()
            }
        }
    }
}

/// Get the package version currently installed if any
pub fn get_installed_version(package_name: &str) -> Option<PackageVersion> {
    let output = Command::new("apt-cache")
        .args(vec!["policy", package_name])
        .env("LANG", "C")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line_prefix = "  Installed: ";
    let package_version_line = output
        .stdout
        .lines()
        .find(|l| l.as_ref().unwrap().starts_with(line_prefix))?
        .ok()?;
    let package_version = package_version_line.split_at(line_prefix.len()).1;

    Some(PackageVersion {
        string: package_version.to_string(),
    })
}

/// Get all version of a package currently in local cache
fn get_cache_package_versions(package_name: String) -> Vec<PackageVersion> {
    glob(&format!(
        "{}{}_*_{}.deb",
        APT_ENV.cache_dir, package_name, APT_ENV.arch
    ))
    .unwrap()
    .filter_map(Result::ok)
    .map(|p| PackageVersion {
        string: p
            .file_name()
            .unwrap()
            .to_os_string()
            .into_string()
            .unwrap()
            .split('_')
            .rev()
            .nth(1)
            .unwrap()
            .to_string(),
    })
    .collect()
}

/// Build apt install command line for a list of packages
pub fn build_install_cmdline(packages: VecDeque<Package>) -> String {
    format!(
        "apt-get install -V --no-install-recommends {}",
        join(
            packages.iter().map(|p| format!(
                "{}{}_{}_{}.deb",
                APT_ENV.cache_dir, p.name, p.version, APT_ENV.arch
            )),
            " "
        )
    )
}
