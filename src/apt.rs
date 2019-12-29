use std::cmp::{Ordering, Reverse};
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

    pub arch: Option<String>,
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

/// Package version constraint
#[derive(Debug)]
pub struct PackageVersionConstaint {
    pub version: PackageVersion,
    pub version_relation: PackageVersionRelation,
}

/// Package dependency
#[derive(Debug)]
pub struct PackageDependency {
    pub package_name: String,

    pub version_constraints: Vec<PackageVersionConstaint>,
}

/// APT environement configuration values
pub struct AptEnv {
    arch: String,
    cache_dir: String,
}

lazy_static! {
    pub static ref APT_ENV: AptEnv = read_apt_env().expect("Unable to read APT environment");
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
        .env("LANG", "C")
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
    apt_env: &AptEnv,
) -> Result<VecDeque<PackageDependency>, Box<dyn error::Error>> {
    let mut deps = VecDeque::new();

    let deb_filepath = format!(
        "{}{}_{}_{}.deb",
        apt_env.cache_dir,
        package.name,
        package.version,
        package
            .arch
            .as_ref()
            .ok_or_else(|| SimpleError::new("Missing package architecture"))?
    );
    let spec = format!("{}={}", package.name, package.version);
    let apt_args = if Path::new(&deb_filepath).is_file() {
        vec!["show", &deb_filepath]
    } else {
        vec!["show", &spec]
    };

    let output = Command::new("apt-cache")
        .args(apt_args)
        .env("LANG", "C")
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

    // TODO parse multiple version constraints for a single package

    for package_desc in package_desc_line
        .split_at(line_prefix.len())
        .1
        .split(',')
        .map(|l| l.trim_start())
    {
        let mut package_desc_tokens = package_desc.split(' ');
        let package_name = package_desc_tokens
            .next()
            .ok_or_else(|| SimpleError::new("Unexpected apt-cache output"))?
            .to_string();
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
                let package_version_raw = &package_desc_tokens
                    .next()
                    .ok_or_else(|| SimpleError::new("Unexpected apt-cache output"))?;
                &package_version_raw[0..&package_version_raw.len() - 1]
            }
        };

        deps.push_back(PackageDependency {
            package_name,
            version_constraints: vec![PackageVersionConstaint {
                version: PackageVersion {
                    string: package_version.to_string(),
                },
                version_relation: package_version_relation,
            }],
        });
    }

    Ok(deps)
}

fn get_dependencies_remote(
    _package: &Package,
    _apt_env: &AptEnv,
) -> Result<VecDeque<PackageDependency>, Box<dyn error::Error>> {
    // TODO Build download dir

    // TODO Check if already downloaded

    // TODO get http://ftp.debian.org/debian/pool/main/c/chromium/chromium_78.0.3904.108-1~deb10u1_amd64.deb

    // TODO get deps from deb

    unimplemented!();
}

/// Get dependencies for a package
pub fn get_dependencies(package: Package, apt_env: &AptEnv) -> VecDeque<PackageDependency> {
    match get_dependencies_cache(&package, &apt_env) {
        Ok(deps) => deps,
        Err(e) => {
            println!(
                "Failed to get dependencies for package {:?} from cache: {}",
                package, e
            );
            get_dependencies_remote(&package, &apt_env).unwrap()
        }
    }
}

/// Find the best package version that satisfies a dependency constraint
pub fn resolve_dependency(
    dependency: &PackageDependency,
    candidates: VecDeque<Package>,
    installed_package: &Option<Package>,
) -> Option<Package> {
    let mut matching_candidates: Box<dyn std::iter::Iterator<Item = &Package>> =
        Box::new(candidates.iter());
    for constraint in &dependency.version_constraints {
        let filter_predicate: Box<dyn Fn(&&Package) -> bool> = match constraint.version_relation {
            PackageVersionRelation::Any => Box::new(|_p| true),
            PackageVersionRelation::StrictlyInferior => {
                Box::new(move |p| p.version < constraint.version)
            }
            PackageVersionRelation::InferiorOrEqual => {
                Box::new(move |p| p.version <= constraint.version)
            }
            PackageVersionRelation::Equal => Box::new(move |p| p.version == constraint.version),
            PackageVersionRelation::SuperiorOrEqual => {
                Box::new(move |p| p.version >= constraint.version)
            }
            PackageVersionRelation::StriclySuperior => {
                Box::new(move |p| p.version > constraint.version)
            }
        };

        matching_candidates = Box::new(matching_candidates.filter(filter_predicate));
    }

    // If installed package matches, return it
    let matching_candidates: Vec<&Package> = matching_candidates.collect();
    if let Some(installed_package) = installed_package {
        if matching_candidates.contains(&installed_package) {
            return Some(installed_package.clone());
        }
    }

    // Return the first match
    Some(matching_candidates[0].clone())
}

/// Get the package version currently installed if any
pub fn get_installed_version(package_name: &str) -> Option<Package> {
    // Get version
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

    // Get architecture
    let output = Command::new("apt-cache")
        .args(vec!["show", package_name])
        .env("LANG", "C")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line_prefix = "Architecture: ";
    let package_arch_line = output
        .stdout
        .lines()
        .find(|l| l.as_ref().unwrap().starts_with(line_prefix))?
        .ok()?;
    let package_arch = package_arch_line.split_at(line_prefix.len()).1;

    Some(Package {
        name: package_name.to_string(),
        version: PackageVersion {
            string: package_version.to_string(),
        },
        arch: Some(package_arch.to_string()),
    })
}

/// Get all version of a package currently in local cache
pub fn get_cache_package_versions(package_name: &str, apt_env: &AptEnv) -> VecDeque<Package> {
    let mut versions = VecDeque::new();

    for arch in &[apt_env.arch.clone(), "all".to_string(), "any".to_string()] {
        for path_entry in glob(&format!(
            "{}{}_*_{}.deb",
            apt_env.cache_dir, package_name, arch
        ))
        .unwrap()
        .filter_map(Result::ok)
        {
            let path = path_entry
                .file_name()
                .unwrap()
                .to_os_string()
                .into_string()
                .unwrap();
            let mut tokens = path.split('_').rev();
            let arch = tokens
                .next()
                .unwrap()
                .split('.')
                .nth(0)
                .unwrap()
                .to_string();
            let version = tokens.next().unwrap().to_string();
            versions.push_back(Package {
                name: package_name.to_string(),
                version: PackageVersion {
                    string: version.to_string(),
                },
                arch: Some(arch.to_string()),
            });
        }
    }

    // Sort
    // TODO find a way to sort a VecDeque inplace
    let mut versions_vec = Vec::from(versions);
    versions_vec.sort_unstable_by_key(|d| Reverse(d.version.clone()));

    VecDeque::from(versions_vec)
}

/// Build apt install command line for a list of packages
pub fn build_install_cmdline(packages: VecDeque<Package>, apt_env: &AptEnv) -> String {
    format!(
        "apt-get install -V --no-install-recommends {}",
        join(
            packages.iter().map(|p| format!(
                "{}{}_{}_{}.deb",
                apt_env.cache_dir,
                p.name,
                p.version,
                p.arch.as_ref().unwrap()
            )),
            " "
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_install_cmdline() {
        let apt_env = AptEnv {
            cache_dir: "/cache/dir/".to_string(),
            arch: "thearch".to_string(),
        };
        let packages: VecDeque<Package> = VecDeque::from(vec![
            Package {
                name: "package1".to_string(),
                version: PackageVersion {
                    string: "1.2.3.4".to_string(),
                },
                arch: Some("thearch".to_string()),
            },
            Package {
                name: "package2".to_string(),
                version: PackageVersion {
                    string: "4.3.2-a1".to_string(),
                },
                arch: Some("all".to_string()),
            },
        ]);
        assert_eq!(
            build_install_cmdline(packages, &apt_env),
            "apt-get install -V --no-install-recommends /cache/dir/package1_1.2.3.4_thearch.deb /cache/dir/package2_4.3.2-a1_all.deb"
        );
    }

    #[test]
    fn test_resolve_dependency() {
        let candidates = VecDeque::from(vec![
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "1.0.3".to_string(),
                },
                arch: Some("4rch".to_string()),
            },
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "1.0.2".to_string(),
                },
                arch: Some("4rch".to_string()),
            },
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "1.0.1".to_string(),
                },
                arch: Some("4rch".to_string()),
            },
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "1.0.0".to_string(),
                },
                arch: Some("4rch".to_string()),
            },
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "0.9.9".to_string(),
                },
                arch: Some("4rch".to_string()),
            },
        ]);

        //
        // Any
        //

        let dependency = PackageDependency {
            package_name: candidates[0].name.clone(),
            version_constraints: vec![PackageVersionConstaint {
                version: candidates[0].version.clone(),
                version_relation: PackageVersionRelation::Any,
            }],
        };
        let installed_package = None;
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[0].clone())
        );

        let installed_package = Some(candidates[3].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[3].clone())
        );

        //
        // StrictlyInferior
        //

        let dependency = PackageDependency {
            package_name: candidates[1].name.clone(),
            version_constraints: vec![PackageVersionConstaint {
                version: candidates[1].version.clone(),
                version_relation: PackageVersionRelation::StrictlyInferior,
            }],
        };
        let installed_package = None;
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[2].clone())
        );

        let installed_package = Some(candidates[3].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[3].clone())
        );

        let installed_package = Some(candidates[0].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[2].clone())
        );

        //
        // InferiorOrEqual
        //

        let dependency = PackageDependency {
            package_name: candidates[1].name.clone(),
            version_constraints: vec![PackageVersionConstaint {
                version: candidates[1].version.clone(),
                version_relation: PackageVersionRelation::InferiorOrEqual,
            }],
        };
        let installed_package = None;
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[1].clone())
        );

        let installed_package = Some(candidates[3].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[3].clone())
        );

        let installed_package = Some(candidates[0].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[1].clone())
        );

        //
        // Equal
        //

        let dependency = PackageDependency {
            package_name: candidates[1].name.clone(),
            version_constraints: vec![PackageVersionConstaint {
                version: candidates[1].version.clone(),
                version_relation: PackageVersionRelation::Equal,
            }],
        };
        let installed_package = None;
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[1].clone())
        );

        let installed_package = Some(candidates[3].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[1].clone())
        );

        //
        // SuperiorOrEqual
        //

        let dependency = PackageDependency {
            package_name: candidates[2].name.clone(),
            version_constraints: vec![PackageVersionConstaint {
                version: candidates[2].version.clone(),
                version_relation: PackageVersionRelation::SuperiorOrEqual,
            }],
        };
        let installed_package = None;
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[0].clone())
        );

        let installed_package = Some(candidates[1].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[1].clone())
        );

        let installed_package = Some(candidates[3].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[0].clone())
        );

        //
        // StriclySuperior
        //

        let dependency = PackageDependency {
            package_name: candidates[2].name.clone(),
            version_constraints: vec![PackageVersionConstaint {
                version: candidates[2].version.clone(),
                version_relation: PackageVersionRelation::StriclySuperior,
            }],
        };
        let installed_package = None;
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[0].clone())
        );

        let installed_package = Some(candidates[1].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[1].clone())
        );

        let installed_package = Some(candidates[2].clone());
        assert_eq!(
            resolve_dependency(&dependency, candidates.clone(), &installed_package),
            Some(candidates[0].clone())
        );
    }
}
