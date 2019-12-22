use std::collections::VecDeque;
use std::error;
use std::io::BufRead;
use std::process::{Command, Stdio};

use clap::{App, Arg};


/// Parsed command line arguments
#[derive(Clone)]
struct CLArgs {
    package_name: String,

    package_version: String,
}


/// A versioned package
#[derive(Debug)]
struct Package {
    name: String,

    version: String,
}


/// Parse and validate command line arguments
fn parse_cl_args() -> CLArgs {
    // Clap arg matching
    let matches = App::new("apt-downgrade")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Downgrade debian packages and their dependencies")
        .author("desbma")
        .arg(Arg::with_name("PACKAGE_NAME").required(true).takes_value(
            true,
        ))
        .arg(
            Arg::with_name("PACKAGE_VERSION")
                .required(true)
                .takes_value(true),
        )
        .get_matches();

    // Post Clap parsing
    let package_name = matches.value_of("PACKAGE_NAME").unwrap().to_string();
    let package_version = matches.value_of("PACKAGE_VERSION").unwrap().to_string();

    // TODO dry run option

    CLArgs {
        package_name,
        package_version,
    }
}


fn get_dependencies_cache(package: &Package) -> Result<VecDeque<Package>, Box<error::Error>> {
    let deps = VecDeque::new();

    let output = Command::new("apt-cache")
        .args(vec![
            "show",
            &format!("{}={}", package.name, package.version),
        ])
        .stderr(Stdio::null())
        .output()?;
    //if !output.status.success() {
    //    return Err(io::Error);
    //}
    let let_line_prefix = "Depends: ";
    let package_desc_line = output
        .stdout
        .lines()
        .filter(|l| l.as_ref().unwrap().starts_with(let_line_prefix))
        .nth(0)
        .unwrap()?;
    for package_desc in package_desc_line
        .split_at(let_line_prefix.len())
        .1
        .split(',')
        .map(|l| l.trim_start())
    {
        println!("{}", package_desc);
    }

    Ok(deps)
}


fn get_dependencies_remote(package: &Package) -> Result<VecDeque<Package>, Box<error::Error>> {
    let deps = VecDeque::new();

    Ok(deps)
}


fn get_dependencies(package: Package) -> VecDeque<Package> {
    match get_dependencies_cache(&package) {
        Ok(deps) => deps,
        Err(e) => {
            println!(
                "Failed to get dependencies for package {:?} from cache: {:?}",
                package,
                e
            );
            get_dependencies_remote(&package).unwrap()
        }
    }
}


fn build_install_cmdline(packages: VecDeque<Package>) -> String {
    "TODO".to_string()
}


fn main() {
    // Parse args
    let cl_args = parse_cl_args();

    // Initial queue states
    let mut to_resolve: VecDeque<Package> = VecDeque::new();
    to_resolve.push_back(Package {
        name: cl_args.package_name,
        version: cl_args.package_version,
    });
    let mut to_install: VecDeque<Package> = VecDeque::new();

    // Resolve packages to install
    while let Some(cur_package) = to_resolve.pop_front() {
        println!("{:?}", cur_package);

        // Get package dependencies
        let deps = get_dependencies(cur_package);
        for dep in deps {
            println!("\t{:?}", dep);
        }

        // TODO if not in build package url

        // TODO download deb (directories-rs cache)

        // TODO get dep for deb
    }

    // TODO install all packages in to_install
    let install_cmdline = build_install_cmdline(to_install);
    println!("Run:\n{}", install_cmdline);
}
