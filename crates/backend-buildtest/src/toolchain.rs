//! What each language's build test needs, whether this machine has it, and the
//! command that installs what's missing on this host (dgtlrift/cddlc#169).
//!
//! A missing toolchain skips that language (or fails it with
//! `CDDLC_REQUIRE_TOOLS`, as CI sets); the install hint is printed once per
//! language, not once per schema. `cddlc-toolchains` prints the same report.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

// ── Languages ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    C,
    Cpp,
    CSharp,
    NodeJs,
    Python,
    Go,
    Java,
    Lua,
    Dart,
}

impl Lang {
    pub const ALL: [Lang; 10] = [
        Lang::Rust,
        Lang::C,
        Lang::Cpp,
        Lang::CSharp,
        Lang::NodeJs,
        Lang::Python,
        Lang::Go,
        Lang::Java,
        Lang::Lua,
        Lang::Dart,
    ];

    /// The name `scripts/interop.sh` and `cddlc generate -l` use.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::C => "c",
            Lang::Cpp => "cpp",
            Lang::CSharp => "csharp",
            Lang::NodeJs => "nodejs",
            Lang::Python => "python",
            Lang::Go => "go",
            Lang::Java => "java",
            Lang::Lua => "lua",
            Lang::Dart => "dart",
        }
    }

    pub fn from_name(name: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| l.name() == name)
    }

    fn display(self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::C => "C",
            Lang::Cpp => "C++",
            Lang::CSharp => "C#",
            Lang::NodeJs => "Node.js",
            Lang::Python => "Python",
            Lang::Go => "Go",
            Lang::Java => "Java",
            Lang::Lua => "Lua",
            Lang::Dart => "Dart",
        }
    }
}

// ── Hosts ─────────────────────────────────────────────────────────────────────

/// The package manager install hints are written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    Apt,
    Dnf,
    Zypper,
    Pacman,
    Apk,
    Brew,
    Choco,
    Unknown,
}

impl Host {
    fn from_name(name: &str) -> Option<Host> {
        Some(match name {
            "apt" => Host::Apt,
            "dnf" | "yum" | "rpm" => Host::Dnf,
            "zypper" => Host::Zypper,
            "pacman" => Host::Pacman,
            "apk" => Host::Apk,
            "brew" => Host::Brew,
            "choco" | "chocolatey" => Host::Choco,
            _ => return None,
        })
    }
}

/// This machine's package manager: `CDDLC_PKG_MANAGER` if set (apt, dnf,
/// zypper, pacman, apk, brew, choco), else from the OS: Homebrew on macOS,
/// Chocolatey on Windows, and on Linux the distribution in `/etc/os-release`
/// (its `ID`, then `ID_LIKE`), or failing that whichever manager is on PATH.
pub fn host() -> Host {
    if let Some(h) = std::env::var("CDDLC_PKG_MANAGER")
        .ok()
        .as_deref()
        .and_then(Host::from_name)
    {
        return h;
    }
    if cfg!(target_os = "macos") {
        return Host::Brew;
    }
    if cfg!(windows) {
        return Host::Choco;
    }
    if let Ok(release) = std::fs::read_to_string("/etc/os-release") {
        if let Some(h) = host_from_os_release(&release) {
            return h;
        }
    }
    for (tool, h) in [
        ("apt-get", Host::Apt),
        ("dnf", Host::Dnf),
        ("zypper", Host::Zypper),
        ("pacman", Host::Pacman),
        ("apk", Host::Apk),
        ("brew", Host::Brew),
    ] {
        if find(tool).is_some() {
            return h;
        }
    }
    Host::Unknown
}

/// The package manager for an `/etc/os-release`, from its `ID` and `ID_LIKE`.
pub fn host_from_os_release(release: &str) -> Option<Host> {
    let field = |key: &str| {
        release
            .lines()
            .find_map(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
            .map(|v| v.trim_matches('"').to_ascii_lowercase())
            .unwrap_or_default()
    };
    let ids = format!("{} {}", field("ID"), field("ID_LIKE"));
    for id in ids.split_whitespace() {
        let h = match id {
            "debian" | "ubuntu" | "linuxmint" | "pop" | "elementary" | "raspbian" | "kali" => {
                Host::Apt
            }
            "fedora" | "rhel" | "centos" | "rocky" | "almalinux" | "ol" | "amzn" => Host::Dnf,
            "suse" | "sles" | "opensuse" | "opensuse-leap" | "opensuse-tumbleweed" => Host::Zypper,
            "arch" | "manjaro" | "endeavouros" => Host::Pacman,
            "alpine" => Host::Apk,
            _ => continue,
        };
        return Some(h);
    }
    None
}

// ── What each language needs ──────────────────────────────────────────────────

/// One thing a build test needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    Cargo,
    Cmake,
    CCompiler,
    CxxCompiler,
    Git,
    DotnetSdk8,
    Node18,
    Npm,
    Python3,
    PythonModules,
    Go,
    Jdk17,
    Maven,
    Lua,
    Busted,
    Dart,
}

impl Need {
    fn describe(self) -> &'static str {
        match self {
            Need::Cargo => "cargo (Rust)",
            Need::Cmake => "cmake",
            Need::CCompiler => "a C compiler (cc)",
            Need::CxxCompiler => "a C++17 compiler (c++)",
            Need::Git => "git (CMake fetches nanocbor with it)",
            Need::DotnetSdk8 => ".NET SDK 8 or later (dotnet)",
            Need::Node18 => "Node.js 18 or later (node)",
            Need::Npm => "npm",
            Need::Python3 => "Python 3 (python3)",
            Need::PythonModules => "the Python packages cbor2>=6.0 and pytest",
            Need::Go => "Go (go)",
            Need::Jdk17 => "a JDK 17 or later (javac)",
            Need::Maven => "Maven (mvn)",
            Need::Lua => "Lua 5.3 or later (lua)",
            Need::Busted => "busted (the Lua test runner)",
            Need::Dart => "the Dart SDK (dart)",
        }
    }
}

/// How to install one need on a host: packages for its package manager, or a
/// command of its own.
enum Install {
    Packages(&'static [&'static str]),
    Command(&'static str),
}

fn install(need: Need, host: Host) -> Install {
    use Host::*;
    use Install::{Command as Cmd, Packages as P};
    match (need, host) {
        (Need::Cargo, Choco) => P(&["rustup.install"]),
        (Need::Cargo, Brew) => P(&["rustup"]),
        (Need::Cargo, _) => Cmd("curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"),

        (Need::Cmake, Unknown) => Cmd("install CMake: https://cmake.org/download/"),
        (Need::Cmake, _) => P(&["cmake"]),

        (Need::CCompiler | Need::CxxCompiler, Apt) => P(&["build-essential"]),
        (Need::CCompiler, Dnf | Zypper) => P(&["gcc", "make"]),
        (Need::CxxCompiler, Dnf | Zypper) => P(&["gcc-c++", "make"]),
        (Need::CCompiler | Need::CxxCompiler, Pacman) => P(&["base-devel"]),
        (Need::CCompiler | Need::CxxCompiler, Apk) => P(&["build-base"]),
        (Need::CCompiler | Need::CxxCompiler, Brew) => Cmd("xcode-select --install"),
        (Need::CCompiler | Need::CxxCompiler, Choco) => P(&["mingw"]),
        (Need::CCompiler | Need::CxxCompiler, Unknown) => Cmd("install gcc or clang (with C++17)"),

        (Need::Git, Unknown) => Cmd("install git: https://git-scm.com/downloads"),
        (Need::Git, _) => P(&["git"]),

        (Need::DotnetSdk8, Apt | Dnf) => P(&["dotnet-sdk-8.0"]),
        (Need::DotnetSdk8, Pacman) => P(&["dotnet-sdk-8.0"]),
        (Need::DotnetSdk8, Apk) => P(&["dotnet8-sdk"]),
        (Need::DotnetSdk8, Choco) => P(&["dotnet-8.0-sdk"]),
        (Need::DotnetSdk8, Brew) => Cmd("brew install --cask dotnet-sdk"),
        (Need::DotnetSdk8, Zypper | Unknown) => Cmd("install the .NET 8 SDK: https://learn.microsoft.com/dotnet/core/install/linux"),

        (Need::Node18 | Need::Npm, Apt | Dnf | Pacman | Apk) => P(&["nodejs", "npm"]),
        (Need::Node18 | Need::Npm, Zypper) => P(&["nodejs-default", "npm-default"]),
        (Need::Node18 | Need::Npm, Brew) => P(&["node"]),
        (Need::Node18 | Need::Npm, Choco) => P(&["nodejs-lts"]),
        (Need::Node18 | Need::Npm, Unknown) => Cmd("install Node.js 22: https://nodejs.org/en/download"),

        (Need::Python3, Apt) => P(&["python3", "python3-venv"]),
        (Need::Python3, Dnf | Zypper | Apk) => P(&["python3"]),
        (Need::Python3, Pacman | Brew | Choco) => P(&["python"]),
        (Need::Python3, Unknown) => Cmd("install Python 3: https://www.python.org/downloads/"),
        (Need::PythonModules, Choco) => Cmd("py -m pip install \"cbor2>=6.0\" pytest"),
        (Need::PythonModules, _) => Cmd(
            "python3 -m venv ~/.venvs/cddlc && ~/.venvs/cddlc/bin/pip install 'cbor2>=6.0' pytest\n\
             export CDDLC_PYTHON=~/.venvs/cddlc/bin/python"),

        (Need::Go, Apt) => P(&["golang-go"]),
        (Need::Go, Dnf) => P(&["golang"]),
        (Need::Go, Zypper | Pacman | Apk | Brew) => P(&["go"]),
        (Need::Go, Choco) => P(&["golang"]),
        (Need::Go, Unknown) => Cmd("install Go: https://go.dev/dl/"),

        (Need::Jdk17, Apt) => P(&["openjdk-17-jdk"]),
        (Need::Jdk17, Dnf | Zypper) => P(&["java-17-openjdk-devel"]),
        (Need::Jdk17, Pacman) => P(&["jdk17-openjdk"]),
        (Need::Jdk17, Apk) => P(&["openjdk17-jdk"]),
        (Need::Jdk17, Brew) => P(&["openjdk@17"]),
        (Need::Jdk17, Choco) => P(&["temurin17"]),
        (Need::Jdk17, Unknown) => Cmd("install a JDK 17: https://adoptium.net/"),
        (Need::Maven, Unknown) => Cmd("install Maven: https://maven.apache.org/download.cgi"),
        (Need::Maven, _) => P(&["maven"]),

        (Need::Lua, Apt) => P(&["lua5.4", "liblua5.4-dev", "luarocks"]),
        (Need::Lua, Dnf) => P(&["lua", "lua-devel", "luarocks"]),
        (Need::Lua, Zypper) => P(&["lua54", "lua54-devel", "lua54-luarocks"]),
        (Need::Lua, Pacman) => P(&["lua", "luarocks"]),
        (Need::Lua, Apk) => P(&["lua5.4", "lua5.4-dev", "luarocks5.4"]),
        (Need::Lua, Brew) => P(&["lua", "luarocks"]),
        (Need::Lua, Choco) => Cmd("Lua and busted are easiest under WSL: wsl --install, then the Linux steps"),
        (Need::Lua, Unknown) => Cmd("install Lua 5.4 and LuaRocks: https://luarocks.org/"),
        (Need::Busted, Choco) => Cmd("luarocks install busted"),
        (Need::Busted, _) => Cmd("luarocks --lua-version 5.4 --local install busted"),

        (Need::Dart, Apt) => Cmd(
            "sudo apt install apt-transport-https\n\
             wget -qO- https://dl-ssl.google.com/linux/linux_signing_key.pub | sudo gpg --dearmor -o /usr/share/keyrings/dart.gpg\n\
             echo 'deb [signed-by=/usr/share/keyrings/dart.gpg arch=amd64] https://storage.googleapis.com/download.dartlang.org/linux/debian stable main' | sudo tee /etc/apt/sources.list.d/dart_stable.list\n\
             sudo apt update && sudo apt install dart"),
        (Need::Dart, Brew) => Cmd("brew tap dart-lang/dart && brew install dart"),
        (Need::Dart, Choco) => P(&["dart-sdk"]),
        (Need::Dart, Pacman) => Cmd("dart is in the AUR: yay -S dart (or https://dart.dev/get-dart)"),
        (Need::Dart, Apk) => Cmd("the Dart SDK needs glibc; Alpine isn't supported: https://dart.dev/get-dart"),
        (Need::Dart, Dnf | Zypper | Unknown) => Cmd("install the Dart SDK: https://dart.dev/get-dart#install-a-zip-file"),
    }
}

/// The commands that install `needs` on `host`: its package manager's packages
/// in one command, then any commands of their own.
pub fn install_commands(needs: &[Need], host: Host) -> Vec<String> {
    let mut packages: Vec<&str> = Vec::new();
    let mut commands: Vec<String> = Vec::new();
    for need in needs {
        match install(*need, host) {
            Install::Packages(ps) => {
                for p in ps {
                    if !packages.contains(p) {
                        packages.push(p);
                    }
                }
            }
            Install::Command(c) => {
                if !commands.iter().any(|x| x == c) {
                    commands.push(c.to_owned());
                }
            }
        }
    }
    let mut out = Vec::new();
    if !packages.is_empty() {
        let pkgs = packages.join(" ");
        out.push(match host {
            Host::Apt => format!("sudo apt install {pkgs}"),
            Host::Dnf => format!("sudo dnf install {pkgs}"),
            Host::Zypper => format!("sudo zypper install {pkgs}"),
            Host::Pacman => format!("sudo pacman -S --needed {pkgs}"),
            Host::Apk => format!("sudo apk add {pkgs}"),
            Host::Brew => format!("brew install {pkgs}"),
            Host::Choco => format!("choco install -y {pkgs}   (in an administrator shell)"),
            Host::Unknown => format!("install: {pkgs}"),
        });
    }
    out.extend(commands);
    out
}

// ── Checking ──────────────────────────────────────────────────────────────────

/// A language whose toolchain is incomplete: what's missing, and how to
/// install it here.
#[derive(Clone, Debug)]
pub struct Missing {
    pub lang: Lang,
    pub needs: Vec<Need>,
    /// Why each need failed (`node v16.20.0 is older than 18`).
    pub details: Vec<String>,
    pub host: Host,
}

impl Missing {
    /// The short form, for one schema's skip reason.
    pub fn short(&self) -> String {
        let what: Vec<&str> = self.needs.iter().map(|n| n.describe()).collect();
        format!(
            "{} toolchain incomplete: {}",
            self.lang.display(),
            what.join(", ")
        )
    }
}

impl fmt::Display for Missing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} build tests need:", self.lang.display())?;
        for d in &self.details {
            writeln!(f, "  - {d}")?;
        }
        writeln!(f, "Install them ({}):", match self.host {
            Host::Apt => "apt", Host::Dnf => "dnf", Host::Zypper => "zypper", Host::Pacman => "pacman",
            Host::Apk => "apk", Host::Brew => "Homebrew", Host::Choco => "Chocolatey",
            Host::Unknown => "package manager not recognized; set CDDLC_PKG_MANAGER to apt, dnf, zypper, pacman, apk, brew or choco",
        })?;
        for c in install_commands(&self.needs, self.host) {
            for line in c.lines() {
                writeln!(f, "    {line}")?;
            }
        }
        Ok(())
    }
}

/// The path of `tool` on PATH.
pub fn find(tool: &str) -> Option<String> {
    let out = Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg(tool)
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned()
    })
}

fn first_of(tools: &[&str]) -> Option<String> {
    tools.iter().find_map(|t| find(t))
}

/// The output of `tool args...`, stdout and stderr together.
fn version_output(tool: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(tool).args(args).output().ok()?;
    Some(format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

/// The first number after `prefix` in `text` (`v18.19.1` → 18 with prefix `v`).
fn major_after(text: &str, prefix: &str) -> Option<u32> {
    let rest = &text[text.find(prefix)? + prefix.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The Python interpreter the Python build tests use: `CDDLC_PYTHON`, else
/// `python3`, else `python`.
pub fn python() -> Option<String> {
    match std::env::var("CDDLC_PYTHON") {
        Ok(p) if !p.is_empty() => Some(p),
        _ => first_of(&["python3", "python"]),
    }
}

/// The busted runner: on PATH, or where `luarocks --local install busted` puts it.
pub fn busted() -> Option<String> {
    if let Some(p) = find("busted") {
        return Some(p);
    }
    let home = std::env::var("HOME").ok()?;
    let local = Path::new(&home).join(".luarocks/bin/busted");
    local.exists().then(|| local.to_string_lossy().into_owned())
}

/// Check `lang`'s toolchain (uncached).
pub fn probe(lang: Lang) -> Result<(), Missing> {
    let mut needs = Vec::new();
    let mut details = Vec::new();
    let mut need = |n: Need, detail: String| {
        if !needs.contains(&n) {
            needs.push(n);
            details.push(detail);
        }
    };
    let missing = |n: Need| format!("{}: not found", n.describe());
    match lang {
        Lang::Rust => {
            if find("cargo").is_none() {
                need(Need::Cargo, missing(Need::Cargo))
            }
        }
        Lang::C | Lang::Cpp => {
            if find("cmake").is_none() {
                need(Need::Cmake, missing(Need::Cmake))
            }
            if lang == Lang::C && first_of(&["cc", "gcc", "clang"]).is_none() {
                need(Need::CCompiler, missing(Need::CCompiler))
            }
            if lang == Lang::Cpp && first_of(&["c++", "g++", "clang++"]).is_none() {
                need(Need::CxxCompiler, missing(Need::CxxCompiler))
            }
            if find("git").is_none() {
                need(Need::Git, missing(Need::Git))
            }
        }
        Lang::CSharp => match find("dotnet") {
            None => need(Need::DotnetSdk8, missing(Need::DotnetSdk8)),
            Some(d) => {
                let sdks = version_output(&d, &["--list-sdks"]).unwrap_or_default();
                let ok = sdks
                    .lines()
                    .any(|l| major_after(l, "").is_some_and(|m| m >= 8));
                if !ok {
                    need(
                        Need::DotnetSdk8,
                        format!(
                            "{}: no SDK 8 or later in `dotnet --list-sdks`",
                            Need::DotnetSdk8.describe()
                        ),
                    )
                }
            }
        },
        Lang::NodeJs => {
            match find("node") {
                None => need(Need::Node18, missing(Need::Node18)),
                Some(n) => {
                    let v = version_output(&n, &["--version"]).unwrap_or_default();
                    if !major_after(&v, "v").is_some_and(|m| m >= 18) {
                        need(
                            Need::Node18,
                            format!(
                                "{}: found {} (Node 22 from https://nodejs.org or NodeSource)",
                                Need::Node18.describe(),
                                v.trim()
                            ),
                        )
                    }
                }
            }
            if find("npm").is_none() {
                need(Need::Npm, missing(Need::Npm))
            }
        }
        Lang::Python => match python() {
            None => need(Need::Python3, missing(Need::Python3)),
            Some(py) => {
                let ok = Command::new(&py)
                    .args([
                        "-c",
                        "import cbor2, pytest, importlib.metadata as m; \
                                  assert int(m.version('cbor2').split('.')[0]) >= 6",
                    ])
                    .output()
                    .is_ok_and(|o| o.status.success());
                if !ok {
                    need(
                        Need::PythonModules,
                        format!(
                            "{}: missing from {py} (CDDLC_PYTHON picks another interpreter)",
                            Need::PythonModules.describe()
                        ),
                    )
                }
            }
        },
        Lang::Go => {
            if find("go").is_none() {
                need(Need::Go, missing(Need::Go))
            }
        }
        Lang::Java => {
            match find("javac") {
                None => need(Need::Jdk17, missing(Need::Jdk17)),
                Some(j) => {
                    let v = version_output(&j, &["-version"]).unwrap_or_default();
                    if !major_after(&v, "javac ").is_some_and(|m| m >= 17) {
                        need(
                            Need::Jdk17,
                            format!("{}: found {}", Need::Jdk17.describe(), v.trim()),
                        )
                    }
                }
            }
            if find("mvn").is_none() {
                need(Need::Maven, missing(Need::Maven))
            }
        }
        Lang::Lua => {
            if first_of(&["lua5.4", "lua54", "lua5.3", "lua"]).is_none() {
                need(Need::Lua, missing(Need::Lua))
            }
            if busted().is_none() {
                need(Need::Busted, missing(Need::Busted))
            }
        }
        Lang::Dart => {
            if find("dart").is_none() {
                need(Need::Dart, missing(Need::Dart))
            }
        }
    }
    if needs.is_empty() {
        Ok(())
    } else {
        Err(Missing {
            lang,
            needs,
            details,
            host: host(),
        })
    }
}

/// Check `lang`'s toolchain, once per process: the first time it's
/// incomplete, the full report and install commands go to stderr; every call
/// returns the short reason for a skip.
pub fn check(lang: Lang) -> Result<(), String> {
    static CACHE: OnceLock<Mutex<HashMap<Lang, Result<(), Missing>>>> = OnceLock::new();
    static REPORTED: OnceLock<Mutex<HashSet<Lang>>> = OnceLock::new();
    let result = {
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cache.entry(lang).or_insert_with(|| probe(lang)).clone()
    };
    result.map_err(|m| {
        let mut reported = REPORTED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if reported.insert(lang) {
            eprintln!("\n{m}");
        }
        m.short()
    })
}

// ── Package registries ────────────────────────────────────────────────────────

/// Output that shows a build failed because a package registry or download
/// was unavailable (a rate limit, a timeout, DNS), not because of the code:
/// the line that says so.
pub fn registry_unavailable(output: &str) -> Option<String> {
    const SIGNS: &[&str] = &[
        "status code: 429",
        "Too Many Requests",
        "429 Too Many",
        "Could not transfer artifact",
        "503 Service Unavailable",
        "502 Bad Gateway",
        "504 Gateway",
        "Could not resolve host",
        "Temporary failure in name resolution",
        "Name or service not known",
        "ETIMEDOUT",
        "ECONNRESET",
        "ECONNREFUSED",
        "EAI_AGAIN",
        "ENOTFOUND",
        "socket hang up",
        "npm ERR! network",
        "npm error network",
        "i/o timeout",
        "TLS handshake timeout",
        "connection reset by peer",
        "dial tcp",
        "Unable to load the service index",
        "NU1301",
        "Got socket error trying to find package",
        "Connection closed before full header",
        "Connection timed out",
        "Failed to connect to",
        "fatal: unable to access",
    ];
    output
        .lines()
        .find(|l| SIGNS.iter().any(|s| l.contains(s)))
        .map(|l| l.trim().chars().take(300).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_from_os_release() {
        assert_eq!(
            host_from_os_release("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Some(Host::Apt)
        );
        assert_eq!(host_from_os_release("ID=fedora\n"), Some(Host::Dnf));
        assert_eq!(
            host_from_os_release("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"),
            Some(Host::Dnf)
        );
        assert_eq!(
            host_from_os_release("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
            Some(Host::Zypper)
        );
        assert_eq!(host_from_os_release("ID=alpine\n"), Some(Host::Apk));
        assert_eq!(host_from_os_release("ID=arch\n"), Some(Host::Pacman));
        assert_eq!(host_from_os_release("ID=plan9\n"), None);
    }

    #[test]
    fn install_commands_group_packages() {
        let apt = install_commands(&[Need::Jdk17, Need::Maven], Host::Apt);
        assert_eq!(apt, ["sudo apt install openjdk-17-jdk maven"]);
        let brew = install_commands(&[Need::Lua, Need::Busted], Host::Brew);
        assert_eq!(
            brew,
            [
                "brew install lua luarocks",
                "luarocks --lua-version 5.4 --local install busted"
            ]
        );
        let choco = install_commands(&[Need::Go], Host::Choco);
        assert_eq!(
            choco,
            ["choco install -y golang   (in an administrator shell)"]
        );
        let apk = install_commands(
            &[Need::CCompiler, Need::CxxCompiler, Need::Cmake],
            Host::Apk,
        );
        assert_eq!(apk, ["sudo apk add build-base cmake"]);
        // A need with no package gets its own command.
        assert!(install_commands(&[Need::Dart], Host::Apt)
            .iter()
            .any(|c| c.contains("download.dartlang.org")));
    }

    #[test]
    fn every_need_has_an_install_on_every_host() {
        let needs = [
            Need::Cargo,
            Need::Cmake,
            Need::CCompiler,
            Need::CxxCompiler,
            Need::Git,
            Need::DotnetSdk8,
            Need::Node18,
            Need::Npm,
            Need::Python3,
            Need::PythonModules,
            Need::Go,
            Need::Jdk17,
            Need::Maven,
            Need::Lua,
            Need::Busted,
            Need::Dart,
        ];
        for host in [
            Host::Apt,
            Host::Dnf,
            Host::Zypper,
            Host::Pacman,
            Host::Apk,
            Host::Brew,
            Host::Choco,
            Host::Unknown,
        ] {
            for n in needs {
                assert!(
                    !install_commands(&[n], host).is_empty(),
                    "{n:?} on {host:?}"
                );
            }
        }
    }

    #[test]
    fn registry_failures_are_recognized() {
        let maven = "[ERROR] Could not transfer artifact x:y:pom:3.4.0 from/to central (https://repo.maven.apache.org/maven2): status code: 429, reason phrase: Too Many Requests (429)";
        assert!(registry_unavailable(maven).is_some());
        assert!(registry_unavailable(
            "npm error network request to https://registry.npmjs.org failed"
        )
        .is_some());
        assert!(registry_unavailable("error[E0308]: mismatched types").is_none());
        assert!(registry_unavailable("AssertionError: expected 1, got 2").is_none());
    }

    #[test]
    fn versions_are_read() {
        assert_eq!(major_after("v18.19.1", "v"), Some(18));
        assert_eq!(major_after("javac 17.0.21", "javac "), Some(17));
        assert_eq!(major_after("8.0.404 [/usr/lib/dotnet/sdk]", ""), Some(8));
    }
}
