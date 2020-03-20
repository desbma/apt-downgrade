apt-downgrade
=============

[![Build status](https://img.shields.io/travis/desbma/apt-downgrade/master.svg?style=flat)](https://travis-ci.org/desbma/apt-downgrade)
[![License](https://img.shields.io/github/license/desbma/apt-downgrade.svg?style=flat)](https://github.com/desbma/apt-downgrade/blob/master/LICENSE)

Downgrade Debian packages safely

** This tool is a work in progress, not ready to be used yet. **


## Features

* Downgrade a package and its dependencies recursively if needed
  - currently installed package are favoured during version resolution
  - otherwise, the most recent version that satisfies the version requirement is chosen
* Safe: all interactions with the system and its packages are done with apt tools (`apt-cache`, `apt-get`...)
* Supports all Debian based distribution (Debian, Ubuntu, etc.)


## Build & install

Clone this repository, and then:

```
cargo build --release
sudo cargo install --path . --root /usr/local
```

## Usage

To downgrade the `chromium` package to version `78.0.3904.108-1`:

```
apt-downgrade chromium 78.0.3904.108-1
```

Run `apt-downgrade -h` to get full command line help.


## TODO

* thread pool for faster execution
* suggest package versions from local cache
* download missing packages if not in local cache
* unit tests


## License

[GPLv3](https://www.gnu.org/licenses/gpl-3.0-standalone.html)
