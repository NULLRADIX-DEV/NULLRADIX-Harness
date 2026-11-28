//! Tells network paths from local ones by their text alone.

/// What a typed path points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathTarget {
    Local,
    /// `\\?\` and `\\.\` paths skip the usual path rules.
    Device,
    /// `\\host\share`, `//host/share` or `\\?\UNC\host\share`, with the host in lowercase.
    Network {
        host: String,
    },
}

const fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

fn split_segment(text: &str) -> (&str, &str) {
    text.split_once(is_separator).unwrap_or((text, ""))
}

fn normalized_host(host: &str) -> String {
    host.trim_end_matches('.').to_lowercase()
}

/// Pure text work, no file is touched.
#[must_use]
pub fn classify_path(path: &str) -> PathTarget {
    let text = path.trim();
    if let Some(rest) = text.strip_prefix(is_separator)
        && let Some(("??", _)) = rest.split_once(is_separator)
    {
        return PathTarget::Device;
    }
    let Some(rest) = text
        .strip_prefix(is_separator)
        .and_then(|rest| rest.strip_prefix(is_separator))
    else {
        return PathTarget::Local;
    };
    let (first, after) = split_segment(rest);
    if first == "?" || first == "." {
        let (kind, tail) = split_segment(after);
        if first == "?" && kind.eq_ignore_ascii_case("unc") {
            let (host, _) = split_segment(tail);
            return PathTarget::Network {
                host: normalized_host(host),
            };
        }
        return PathTarget::Device;
    }
    PathTarget::Network {
        host: normalized_host(first),
    }
}

/// Hosts allowed as network shares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkShares {
    hosts: Vec<String>,
}

impl NetworkShares {
    /// Entries are host names, with or without slashes around them, in any letter case.
    #[must_use]
    pub fn new<S: AsRef<str>>(hosts: impl IntoIterator<Item = S>) -> Self {
        let hosts = hosts
            .into_iter()
            .map(|host| {
                let trimmed = host.as_ref().trim().trim_start_matches(is_separator);
                normalized_host(split_segment(trimmed).0)
            })
            .filter(|host| !host.is_empty())
            .collect();
        Self { hosts }
    }

    /// Whether `path` reaches an unlisted machine or a device.
    #[must_use]
    pub fn refuses(&self, path: &str) -> bool {
        match classify_path(path) {
            PathTarget::Local => false,
            PathTarget::Device => true,
            PathTarget::Network { host } => !self.hosts.contains(&host),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(host: &str) -> PathTarget {
        PathTarget::Network {
            host: host.to_owned(),
        }
    }

    #[test]
    fn local_paths_stay_local() {
        for path in [
            "",
            "   ",
            "relative/dir",
            "/home/me/repo",
            "\\folder",
            "C:\\work\\repo",
            "C:/work/repo",
            "~/repo",
            "/",
            "a//b",
            "folder\\\\name",
        ] {
            assert_eq!(classify_path(path), PathTarget::Local, "{path:?}");
        }
    }

    #[test]
    fn two_leading_separators_name_a_host_in_either_spelling() {
        assert_eq!(classify_path("\\\\server\\share\\repo"), network("server"));
        assert_eq!(classify_path("//server/share/repo"), network("server"));
        assert_eq!(classify_path("\\/server/share"), network("server"));
        assert_eq!(classify_path("/\\server\\share"), network("server"));
        assert_eq!(classify_path("  //server/share"), network("server"));
        assert_eq!(classify_path("//server"), network("server"));
    }

    #[test]
    fn the_host_ignores_case_and_a_trailing_dot() {
        assert_eq!(
            classify_path("//FileServer.Corp./x"),
            network("fileserver.corp")
        );
        assert_eq!(classify_path("\\\\SERVER\\x"), network("server"));
    }

    #[test]
    fn three_separators_leave_no_host_to_allow() {
        assert_eq!(classify_path("///tmp/x"), network(""));
        assert_eq!(classify_path("\\\\\\x"), network(""));
        assert_eq!(classify_path("//"), network(""));
    }

    #[test]
    fn device_prefixes_are_never_a_host() {
        for path in [
            "\\\\?\\C:\\work",
            "//?/C:/work",
            "\\\\.\\pipe\\name",
            "//./pipe/name",
            "\\\\?\\",
            "\\\\.",
            "\\\\?\\Volume{1234}\\x",
            "\\??\\C:\\work",
        ] {
            assert_eq!(classify_path(path), PathTarget::Device, "{path:?}");
        }
    }

    #[test]
    fn a_unc_path_in_device_form_names_its_host() {
        assert_eq!(
            classify_path("\\\\?\\UNC\\Server\\share\\x"),
            network("server")
        );
        assert_eq!(
            classify_path("\\\\?\\unc\\server\\share"),
            network("server")
        );
        assert_eq!(classify_path("//?/UNC/server/share"), network("server"));
        assert_eq!(classify_path("\\\\?\\UNC\\"), network(""));
        assert_eq!(
            classify_path("\\\\.\\UNC\\server\\share"),
            PathTarget::Device
        );
    }

    #[test]
    fn a_host_that_only_looks_like_a_prefix_is_a_host() {
        assert_eq!(classify_path("\\\\?x\\share"), network("?x"));
        assert_eq!(classify_path("\\\\..\\share"), network(""));
    }

    #[test]
    fn nothing_but_listed_hosts_is_let_through() {
        let shares = NetworkShares::new(["FileServer", " //nas/ ", "", "\\\\files.corp\\share"]);
        assert!(!shares.refuses("/home/me/repo"));
        assert!(!shares.refuses("C:\\repo"));
        assert!(!shares.refuses("//fileserver/team/repo"));
        assert!(!shares.refuses("\\\\FILESERVER\\team\\repo"));
        assert!(!shares.refuses("//NAS/x"));
        assert!(!shares.refuses("\\\\files.corp\\x"));
        assert!(!shares.refuses("\\\\?\\UNC\\nas\\x"));
        assert!(shares.refuses("//attacker.example/x"));
        assert!(shares.refuses("//fileserver.evil/x"));
        assert!(shares.refuses("//nas@80/x"));
        assert!(shares.refuses("\\\\?\\C:\\repo"));
        assert!(shares.refuses("\\\\.\\pipe\\x"));
        assert!(shares.refuses("///x"));
    }

    #[test]
    fn without_a_list_every_network_path_is_refused() {
        let none = NetworkShares::default();
        assert!(none.refuses("//server/share"));
        assert!(none.refuses("\\\\server\\share"));
        assert!(none.refuses("\\\\?\\C:\\x"));
        assert!(!none.refuses("/srv/repo"));
        assert!(NetworkShares::new(["", "  ", "//"]).refuses("//server/share"));
    }
}
