apt-downgrade
=============

[![Build status](https://img.shields.io/travis/desbma/apt-downgrade/master.svg?style=flat)](https://travis-ci.org/desbma/apt-downgrade)
[![License](https://img.shields.io/github/license/desbma/apt-downgrade.svg?style=flat)](https://github.com/desbma/apt-downgrade/blob/master/LICENSE)

Downgrade Debian packages safely


## Features

* Downgrade a package and its dependencies recursively if needed
  - currently installed package are favoured during version resolution
  - otherwise, the most recent version that satisfies the version requirement is chosen
* Safe: all interaction with the system are done with apt tools (`apt-cache`, `apt-get`...)
* Supports all Debian based distribution (Debian, Ubuntu, etc.)


## Build & install

Clone this repository, and then:

```
cargo build --release
sudo cargo install --path . --root /usr/local
```


## TODO

* thread pool for faster execution
* download missing packages if not in local cache
* unit tests


## License

[GPLv3](https://www.gnu.org/licenses/gpl-3.0-standalone.html)