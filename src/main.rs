use std::cmp::Ordering;
use std::collections::VecDeque;
use std::error;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

use clap::{App, Arg};
use glob::glob;
use itertools::join;

const ARCH: &str = "amd64"; // TODO get from command line/env

#[derive(Debug, Clone, Eq, PartialEq)]
struct PackageVersion {
    string: String,
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

/// Parsed command line arguments
#[derive(Clone)]
struct CLArgs {
    package_name: String,

    package_version: PackageVersion,

    dry_run: bool,
}

/// A versioned package
#[derive(Clone, Debug, PartialEq)]
struct Package {
    name: String,

    version: PackageVersion,
}

#[derive(Debug)]
enum PackageVersionRelation {
    Any,
    StrictlyInferior,
    InferiorOrEqual,
    Equal,
    SuperiorOrEqual,
    StriclySuperior,
}

/// Package dependency
#[derive(Debug)]
struct PackageDependency {
    package: Package,

    version_relation: PackageVersionRelation,
}

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

/// Parse and validate command line arguments
fn parse_cl_args() -> CLArgs {
    // Clap arg matching
    let matches = App::new("apt-downgrade")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Downgrade debian packages and their dependencies")
        .author("desbma")
        .arg(
            Arg::with_name("PACKAGE_NAME")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("PACKAGE_VERSION")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("DRY_RUN")
                .short("d")
                .long("dry-run")
                .help("Only display install command, but do not install anything"),
        )
        .get_matches();

    // Post Clap parsing
    let package_name = matches.value_of("PACKAGE_NAME").unwrap().to_string();
    let package_version = matches.value_of("PACKAGE_VERSION").unwrap();
    let dry_run = matches.is_present("DRY_RUN");

    CLArgs {
        package_name,
        package_version: PackageVersion {
            string: package_version.to_string(),
        },
        dry_run,
    }
}

fn get_dependencies_cache(
    package: &Package,
) -> Result<VecDeque<PackageDependency>, Box<dyn error::Error>> {
    let mut deps = VecDeque::new();

    // TODO allow passing cache dir from command line
    let deb_filepath = format!(
        "/var/cache/apt/archives/{}_{}_{}.deb",
        package.name, package.version, ARCH
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
        .filter(|l| l.as_ref().unwrap().starts_with(line_prefix))
        .nth(0)
        .unwrap()?;
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
                let package_version_raw = &package_desc_tokens.next().unwrap();
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

fn get_dependencies(package: Package) -> VecDeque<PackageDependency> {
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

fn resolve_version(
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
                && (installed_version.as_ref().unwrap() < &dependency.package.version)
            {
                installed_version.clone()
            } else {
                version_candidates
                    .iter()
                    .find(|v| v < &&dependency.package.version)
                    .cloned()
            }
        }
        PackageVersionRelation::InferiorOrEqual => {
            if installed_version.is_some()
                && (installed_version.as_ref().unwrap() <= &dependency.package.version)
            {
                installed_version.clone()
            } else {
                version_candidates
                    .iter()
                    .find(|v| v <= &&dependency.package.version)
                    .cloned()
            }
        }
        PackageVersionRelation::Equal => version_candidates
            .iter()
            .find(|v| v == &&dependency.package.version)
            .cloned(),
        PackageVersionRelation::SuperiorOrEqual => {
            if installed_version.is_some()
                && (installed_version.as_ref().unwrap() >= &dependency.package.version)
            {
                installed_version.clone()
            } else {
                version_candidates
                    .iter()
                    .find(|v| v >= &&dependency.package.version)
                    .cloned()
            }
        }
        PackageVersionRelation::StriclySuperior => {
            if installed_version.is_some()
                && (installed_version.as_ref().unwrap() > &dependency.package.version)
            {
                installed_version.clone()
            } else {
                version_candidates
                    .iter()
                    .find(|v| v > &&dependency.package.version)
                    .cloned()
            }
        }
    }
}

fn get_installed_version(package_name: &str) -> Option<PackageVersion> {
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
        .filter(|l| l.as_ref().unwrap().starts_with(line_prefix))
        .nth(0)?
        .ok()?;
    let package_version = package_version_line.split_at(line_prefix.len()).1;

    Some(PackageVersion {
        string: package_version.to_string(),
    })
}

fn get_cache_package_versions(package_name: String) -> Vec<PackageVersion> {
    glob(&format!(
        "/var/cache/apt/archives/{}_*_{}.deb",
        package_name, ARCH
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

fn build_install_cmdline(packages: VecDeque<Package>) -> String {
    format!(
        "apt-get install -V --no-install-recommends {}",
        join(packages.iter().map(|p| format!(
            "/var/cache/apt/archives/{}_{}_{}.deb",
            p.name, p.version, ARCH
        )), " ")
    )
}

fn main() {
    // Parse args
    let cl_args = parse_cl_args();

    // Initial queue states
    let mut to_resolve: VecDeque<PackageDependency> = VecDeque::new();
    to_resolve.push_back(PackageDependency {
        package: Package {
            name: cl_args.package_name,
            version: cl_args.package_version,
        },
        version_relation: PackageVersionRelation::Equal,
    });
    let mut to_install: VecDeque<Package> = VecDeque::new();

    print!("Analyzing dependencies...");
    io::stdout().flush().unwrap();

    // Resolve packages to install
    let mut progress = 0;
    while let Some(dependency) = to_resolve.pop_front() {
        // Resolve version
        let installed_version = get_installed_version(&dependency.package.name);
        let resolved_version = resolve_version(&dependency, &installed_version)
            .unwrap_or_else(|| panic!("Unable to resolve dependency {:?}", dependency));
        let package = Package {
            name: dependency.package.name,
            version: resolved_version,
        };

        progress = progress + 1;
        print!("\rAnalyzing {} dependencies...", progress);
        io::stdout().flush().unwrap();

        // Already in install queue?
        if to_install.contains(&package) {
            continue;
        }

        // Already installed?
        if let Some(installed_version) = installed_version {
            if installed_version == package.version {
                continue;
            }
        }

        // Add to install queue
        to_install.push_back(package.clone());

        // Get package dependencies
        let mut deps = get_dependencies(package);
        to_resolve.append(&mut deps);
    }
    println("");

    // Install
    if to_install.is_empty() {
        println!("Nothing to do");
    } else {
        let install_cmdline = build_install_cmdline(to_install);
        if cl_args.dry_run {
            println!("{}", install_cmdline);
        } else {
            unimplemented!();
        }
    }
}
