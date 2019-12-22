use std::collections::VecDeque;
use std::error;
use std::fmt;
use std::io::BufRead;
use std::os::unix::process::ExitStatusExt;
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


#[derive(Debug)]
enum PackageVersionRelation {
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

    relation: PackageVersionRelation,
}


#[derive(Debug)]
struct CommandError {
    status:  std::process::ExitStatus
}


impl fmt::Display for CommandError  {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status.code() {
            Some(code) => write!(f, "Command returned {}", code),
            None => write!(f, "Command killed by signal {}", self.status.signal().unwrap())
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


fn get_dependencies_cache(package: &Package) -> Result<VecDeque<Package>, Box<dyn error::Error>> {
    let deps = VecDeque::new();

    let output = Command::new("apt-cache")
        .args(vec![
            "show",
            &format!("{}={}", package.name, package.version),
        ])
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
       return Err(Box::new(CommandError {status: output.status}));
    }
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
        let mut package_desc_tokens = package_desc.split(' ');
        let package_name = &package_desc_tokens.next().unwrap();
        let package_version_relation_raw = &package_desc_tokens.next().unwrap()[1..];
        let package_version_relation = match package_version_relation_raw {
            "<<" => PackageVersionRelation::StrictlyInferior,
            "<=" => PackageVersionRelation::InferiorOrEqual,
            "==" => PackageVersionRelation::Equal,
            ">=" => PackageVersionRelation::SuperiorOrEqual,
            ">>" => PackageVersionRelation::StriclySuperior,
            r => {
                panic!("Unexpected version relation: {}", r)
            }
        };
        let package_version_raw = &package_desc_tokens.next().unwrap();
        let package_version = &package_version_raw[0..&package_version_raw.len() - 1];
        println!("{} {:?} {}", &package_name, &package_version_relation, &package_version);
    }

    Ok(deps)
}


fn get_dependencies_remote(_package: &Package) -> Result<VecDeque<Package>, Box<dyn error::Error>> {
    unimplemented!();
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


fn build_install_cmdline(_packages: VecDeque<Package>) -> String {
    unimplemented!();
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
