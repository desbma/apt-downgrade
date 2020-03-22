use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::error;
use std::fmt;
use std::fs;
use std::fs::File;
use std::io::{copy, BufRead};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

use directories::ProjectDirs;
use glob::glob;
use itertools::join;
use scraper::{Html, Selector};
use simple_error::SimpleError;

/// Package version with comparison traits
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
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

    pub filepath: Option<String>,

    pub url: Option<String>,
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

impl fmt::Display for PackageDependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for version_constraint in &self.version_constraints {
            match version_constraint.version_relation {
                PackageVersionRelation::Any => {
                    write!(f, "{}", self.package_name)?;
                }
                PackageVersionRelation::StrictlyInferior => {
                    write!(f, "{}<<{}", self.package_name, version_constraint.version)?;
                }
                PackageVersionRelation::InferiorOrEqual => {
                    write!(f, "{}<={}", self.package_name, version_constraint.version)?;
                }
                PackageVersionRelation::Equal => {
                    write!(f, "{}={}", self.package_name, version_constraint.version)?;
                }
                PackageVersionRelation::SuperiorOrEqual => {
                    write!(f, "{}>={}", self.package_name, version_constraint.version)?;
                }
                PackageVersionRelation::StriclySuperior => {
                    write!(f, "{}>>{}", self.package_name, version_constraint.version)?;
                }
            }
        }

        Ok(())
    }
}

/// APT environement configuration values
pub struct AptEnv {
    arch: String,
    cache_dir: String,
    // TODO add distro & release
}

/// Read APT environment values
pub fn read_apt_env() -> Result<AptEnv, Box<dyn error::Error>> {
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
    let lines: Vec<String> = output.stdout.lines().filter_map(Result::ok).collect();
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
    cmd: Vec<String>,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Command {} ", join(&self.cmd, " "))?;
        match self.status.code() {
            Some(code) => write!(f, "returned {}", code),
            None => write!(f, "killed by signal {}", self.status.signal().unwrap()),
        }
    }
}

impl error::Error for CommandError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

fn download_package(package: &mut Package) -> Result<(), Box<dyn error::Error>> {
    // Build target dir
    let dirs = ProjectDirs::from("", "Desbma", "APT Downgrade")
        .ok_or_else(|| SimpleError::new("Unable to compute cache dir"))?;
    let cache_dir = dirs.cache_dir();
    fs::create_dir_all(cache_dir)?;

    // Build target filepath
    let url = package.url.as_ref().unwrap();
    let filename = url
        .rsplit('/')
        .next()
        .ok_or_else(|| SimpleError::new("Unable to extract filename from URL"))?;
    let filepath_final = cache_dir.join(&filename);

    if filepath_final.exists() {
        info!("Got {:?} from cache in {:?}", url, filepath_final);
    } else {
        // Download
        info!("Downloading {:?} to {:?}", url, filepath_final);
        let mut response = reqwest::blocking::get(url)?.error_for_status()?;
        let filepath_tmp = cache_dir.join(format!("{}.tmp", filename));
        let mut target_file = File::create(&filepath_tmp)?;
        copy(&mut response, &mut target_file)?;
        drop(target_file);
        fs::rename(&filepath_tmp, &filepath_final)?;
    }

    // Set filepath
    package.filepath = Some(
        filepath_final
            .into_os_string()
            .into_string()
            .or_else(|_| Err(SimpleError::new("Unexpected filename")))?,
    );

    // All good
    Ok(())
}

/// Get dependencies for a package
pub fn get_dependencies(
    mut package: &mut Package,
) -> Result<Vec<PackageDependency>, Box<dyn error::Error>> {
    let mut deps = Vec::new();

    if package.filepath.is_none() {
        download_package(&mut package)?;
    }

    let deb_filepath = package.filepath.as_ref().unwrap();
    let spec = format!("{}={}", package.name, package.version);
    let apt_args = if Path::new(&deb_filepath).is_file() {
        vec!["show", &deb_filepath]
    } else {
        vec!["show", &spec]
    };

    let output = Command::new("apt-cache")
        .args(&apt_args)
        .env("LANG", "C")
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        let mut cmd: Vec<String> = vec!["apt-cache".to_string()];
        cmd.extend(apt_args.iter().map(|s| (*s).to_string()));
        return Err(Box::new(CommandError {
            status: output.status,
            cmd,
        }));
    }
    let line_prefix = "Depends: ";
    let package_desc_line = output
        .stdout
        .lines()
        .filter_map(Result::ok)
        .find(|l| l.starts_with(line_prefix))
        .ok_or_else(|| SimpleError::new("Unexpected apt-cache output"))?;

    // TODO parse multiple version constraints for a single package

    for package_desc in package_desc_line
        .split_at(line_prefix.len())
        .1
        .split(',')
        .map(|l| l.trim_start())
    {
        let mut package_desc_tokens = package_desc
            .split('|') // TODO handle 'or' constraints
            .next()
            .ok_or_else(|| SimpleError::new("Unexpected apt-cache output"))?
            .trim_end()
            .split(' ');
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
                    .rsplit(':')
                    .next()
                    .ok_or_else(|| SimpleError::new("Unexpected apt-cache output"))?
            }
        };

        deps.push(PackageDependency {
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

/// Find the best package version that satisfies a dependency constraint
pub fn resolve_dependency(
    dependency: &PackageDependency,
    candidates: Vec<Package>,
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
    matching_candidates.get(0).cloned().cloned()
}

/// Get the package version currently installed if any
pub fn get_installed_version(package_name: &str, apt_env: &AptEnv) -> Option<Package> {
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
        .filter_map(Result::ok)
        .find(|l| l.starts_with(line_prefix))?;
    let package_version = package_version_line
        .split_at(line_prefix.len())
        .1
        .rsplit(':')
        .next()?;
    if package_version == "(none)" {
        return None;
    }

    // Get filename
    let output = Command::new("apt-cache")
        .args(vec!["show", package_name])
        .env("LANG", "C")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let lines: Vec<String> = output.stdout.lines().filter_map(Result::ok).collect();
    let line_prefix = "Filename: ";
    let package_filename_line = lines.iter().find(|l| l.starts_with(line_prefix))?;
    let package_filename = Path::new(package_filename_line.split_at(line_prefix.len()).1)
        .file_name()?
        .to_str()?
        .to_string();

    // Get architecture
    let line_prefix = "Architecture: ";
    let package_arch_line = lines.iter().find(|l| l.starts_with(line_prefix))?;
    let package_arch = package_arch_line.split_at(line_prefix.len()).1;

    debug!(
        "Installed version for {}: {} ({})",
        package_name, package_version, package_arch
    );

    Some(Package {
        name: package_name.to_string(),
        version: PackageVersion {
            string: package_version.to_string(),
        },
        arch: Some(package_arch.to_string()),
        filepath: Some(format!("{}{}", apt_env.cache_dir, package_filename)),
        url: None,
    })
}

/// Get all versions of a package currently in local cache
pub fn get_cache_package_versions(
    package_name: &str,
    apt_env: &AptEnv,
) -> Result<Vec<Package>, Box<dyn error::Error>> {
    let mut versions = Vec::new();

    for arch in &[apt_env.arch.clone(), "all".to_string(), "any".to_string()] {
        for path_entry in glob(&format!(
            "{}{}_*_{}.deb",
            apt_env.cache_dir, package_name, arch
        ))?
        .filter_map(Result::ok)
        {
            let path = path_entry
                .file_name()
                .ok_or_else(|| {
                    SimpleError::new(format!("Unexpected entry in {}", apt_env.cache_dir))
                })?
                .to_os_string()
                .into_string()
                .or_else(|_| {
                    Err(SimpleError::new(format!(
                        "Unexpected entry in {}",
                        apt_env.cache_dir
                    )))
                })?;
            let mut tokens = path.split('_').rev();
            let arch = tokens
                .next()
                .ok_or_else(|| SimpleError::new(format!("Unexpected package filename: {}", path)))?
                .split('.')
                .next()
                .ok_or_else(|| SimpleError::new(format!("Unexpected package filename: {}", path)))?
                .to_string();
            let mut version = tokens
                .next()
                .ok_or_else(|| SimpleError::new(format!("Unexpected package filename: {}", path)))?
                .replace("%3a", ":"); // TODO better urlescape
            version = version
                .rsplit(':')
                .next()
                .ok_or_else(|| SimpleError::new(format!("Unexpected package filename: {}", path)))?
                .to_string();
            debug!("Local version for {}: {} ({})", package_name, version, arch);
            versions.push(Package {
                name: package_name.to_string(),
                version: PackageVersion {
                    string: version.to_string(),
                },
                arch: Some(arch.to_string()),
                filepath: Some(
                    path_entry
                        .into_os_string()
                        .into_string()
                        .or_else(|_| Err(SimpleError::new("Unable to convert OS string")))?,
                ),
                url: None,
            });
        }
    }

    Ok(versions)
}

pub fn get_package_index_url(
    package_name: &str,
    apt_env: &AptEnv,
) -> Result<String, Box<dyn error::Error>> {
    // TODO choose URL from distro
    let mirrors_url = format!(
        "https://packages.debian.org/sid/{}/{}/download",
        apt_env.arch, package_name
    );

    // Download
    debug!("GET {}", mirrors_url);
    let html = reqwest::blocking::get(&mirrors_url)?
        .error_for_status()?
        .text()?;

    // Parse
    let document = Html::parse_document(&html);
    let selector = Selector::parse("a").unwrap();
    let mut url = document
        .select(&selector)
        .map(|e| e.value().attr("href").unwrap())
        .find(|u| u.starts_with("http://ftp.debian.org/debian/pool/"))
        .ok_or_else(|| SimpleError::new("Unexpected HTML"))?
        .rsplitn(2, '/')
        .nth(1)
        .ok_or_else(|| SimpleError::new("Unexpected HTML"))?
        .to_string();
    url.push('/');

    Ok(url)
}

/// Get all versions of a package from remote API
pub fn get_remote_package_versions(
    package_name: &str,
    html_cache: &mut HashMap<String, String>,
    apt_env: &AptEnv,
) -> Result<Vec<Package>, Box<dyn error::Error>> {
    let mut packages = Vec::new();

    // Notes:
    // * using directly index like http://ftp.debian.org/debian/pool/main/libr/libreoffice/
    // is not reliable because directory is sometimes hard to deduce from package (ie. libasound2 is in alsa-lib dir)
    // * the API at https://sources.debian.org/doc/api/ is incomplete so useless for our needs

    // Get index URL
    let index_url = get_package_index_url(package_name, apt_env)?;

    // Download
    let html = match html_cache.entry(index_url.clone()) {
        Entry::Occupied(h) => {
            trace!("Got {} from HTML cache", index_url);
            h.get().clone()
        }
        Entry::Vacant(e) => {
            debug!("GET {}", index_url);
            let html = reqwest::blocking::get(&index_url)?
                .error_for_status()?
                .text()?;
            e.insert(html.clone());
            html
        }
    };

    // Parse
    let document = Html::parse_document(&html);
    let selector = Selector::parse("a").unwrap();
    let filename_prefix = format!("{}_", package_name);
    let arch_whitelist = [&apt_env.arch, "all", "any"];
    for filename in document
        .select(&selector)
        .map(|e| e.value().attr("href").unwrap())
        .filter(|u| u.starts_with(&filename_prefix) && u.ends_with(".deb"))
    {
        let filename_noext = Path::new(filename)
            .file_stem()
            .unwrap()
            .to_os_string()
            .into_string()
            .unwrap();
        let mut tokens = filename_noext.rsplit('_');
        let arch = tokens.next().unwrap();
        if !arch_whitelist.contains(&arch) {
            continue;
        }
        let version = tokens.next().unwrap();
        debug!(
            "Remote version for {}: {} ({})",
            package_name, version, arch
        );
        packages.push(Package {
            name: package_name.to_string(),
            version: PackageVersion {
                string: version.to_string(),
            },
            arch: Some(arch.to_string()),
            filepath: None,
            url: Some(format!("{}{}", index_url, filename)),
        });
    }

    Ok(packages)
}

/// Build apt install command line for a list of packages
pub fn build_install_cmdline(packages: Vec<Package>) -> Vec<String> {
    let mut cmd = vec![
        "apt-get".to_string(),
        "install".to_string(),
        "-V".to_string(),
        "--no-install-recommends".to_string(),
    ];
    cmd.extend(
        packages
            .iter()
            .map(|p| p.filepath.as_ref().unwrap().clone()),
    );
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_install_cmdline() {
        let packages: Vec<Package> = vec![
            Package {
                name: "package1".to_string(),
                version: PackageVersion {
                    string: "1.2.3.4".to_string(),
                },
                arch: None,
                filepath: Some("/p1".to_string()),
                url: None,
            },
            Package {
                name: "package2".to_string(),
                version: PackageVersion {
                    string: "4.3.2-a1".to_string(),
                },
                arch: None,
                filepath: Some("/p2".to_string()),
                url: None,
            },
        ];
        assert_eq!(
            build_install_cmdline(packages),
            vec![
                "apt-get",
                "install",
                "-V",
                "--no-install-recommends",
                "/p1",
                "/p2"
            ]
        );
    }

    #[test]
    fn test_resolve_dependency() {
        let candidates = vec![
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "1.0.3".to_string(),
                },
                arch: None,
                filepath: None,
                url: None,
            },
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "1.0.2".to_string(),
                },
                arch: None,
                filepath: None,
                url: None,
            },
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "1.0.1".to_string(),
                },
                arch: None,
                filepath: None,
                url: None,
            },
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "1.0.0".to_string(),
                },
                arch: None,
                filepath: None,
                url: None,
            },
            Package {
                name: "p1".to_string(),
                version: PackageVersion {
                    string: "0.9.9".to_string(),
                },
                arch: None,
                filepath: None,
                url: None,
            },
        ];

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

    #[test]
    fn test_get_remote_package_versions() {
        let apt_env = AptEnv {
            arch: "amd64".to_string(),
            cache_dir: "/tmp".to_string(),
        };
        let mut html_cache: HashMap<String, String> = HashMap::new();
        let r = get_remote_package_versions("libreoffice", &mut html_cache, &apt_env);
        assert!(r.is_ok());
        let packages = r.unwrap();
        assert!(packages.len() > 1);
        for package in packages {
            assert_eq!(package.name, "libreoffice");
            assert!(package.arch.is_some());
            assert!(package.filepath.is_none());
            assert!(package.url.is_some());
            let url = package.url.unwrap();
            assert!(url.starts_with("http"));
            assert!(url.ends_with(".deb"));
        }
    }

    #[test]
    fn test_get_package_index_url() {
        let apt_env = AptEnv {
            arch: "amd64".to_string(),
            cache_dir: "/tmp".to_string(),
        };

        let r = get_package_index_url("libreoffice", &apt_env);
        assert!(r.is_ok());
        assert_eq!(
            r.unwrap(),
            "http://ftp.debian.org/debian/pool/main/libr/libreoffice/"
        );

        let r = get_package_index_url("libasound2", &apt_env);
        assert!(r.is_ok());
        assert_eq!(
            r.unwrap(),
            "http://ftp.debian.org/debian/pool/main/a/alsa-lib/"
        );
    }
}
