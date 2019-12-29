use std::collections::VecDeque;
use std::io::{self, Write};

use clap::{App, Arg};

#[macro_use]
extern crate lazy_static;

mod apt;

/// Parsed command line arguments
#[derive(Clone)]
struct CLArgs {
    package_name: String,

    package_version: apt::PackageVersion,

    dry_run: bool,
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
        package_version: apt::PackageVersion {
            string: package_version.to_string(),
        },
        dry_run,
    }
}

fn main() {
    // Parse args
    let cl_args = parse_cl_args();

    // Initial queue states
    let mut to_resolve: VecDeque<apt::PackageDependency> = VecDeque::new();
    to_resolve.push_back(apt::PackageDependency {
        package_name: cl_args.package_name,
        version_constraints: vec![apt::PackageVersionConstaint {
            version: cl_args.package_version,
            version_relation: apt::PackageVersionRelation::Equal,
        }],
    });
    let mut to_install: VecDeque<apt::Package> = VecDeque::new();

    print!("Analyzing dependencies...");
    io::stdout().flush().unwrap();

    // Resolve packages to install
    let mut progress = 0;
    while let Some(dependency) = to_resolve.pop_front() {
        // Resolve version
        let installed_package = apt::get_installed_version(&dependency.package_name);
        let package_candidates =
            apt::get_cache_package_versions(&dependency.package_name, &apt::APT_ENV);
        // TODO add remote versions
        let resolved_package =
            apt::resolve_dependency(&dependency, package_candidates, &installed_package)
                .unwrap_or_else(|| panic!("Unable to resolve dependency {:?}", dependency));

        progress += 1;
        print!("\rAnalyzing {} dependencies...", progress);
        io::stdout().flush().unwrap();

        // Already in install queue?
        if to_install.contains(&resolved_package) {
            continue;
        }

        // Already installed?
        if let Some(installed_package) = installed_package {
            if installed_package == resolved_package {
                continue;
            }
        }

        // Add to install queue
        to_install.push_back(resolved_package.clone());

        // Get package dependencies
        let mut deps = apt::get_dependencies(resolved_package, &apt::APT_ENV);
        to_resolve.append(&mut deps);
    }
    println!();

    // Install
    if to_install.is_empty() {
        println!("Nothing to do");
    } else {
        let install_cmdline = apt::build_install_cmdline(to_install, &apt::APT_ENV);
        if cl_args.dry_run {
            println!("Run:\n{}", install_cmdline);
        } else {
            unimplemented!();
        }
    }
}
