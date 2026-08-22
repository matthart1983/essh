//! Percent-token expansion for `ssh_config` values.
//!
//! `ProxyCommand`, `ControlPath`, `IdentityFile` and friends are written with
//! tokens rather than literals — `ProxyCommand cloudflared access ssh
//! --hostname %h` is useless without expansion, and `ControlPath
//! ~/.ssh/cm-%r@%h:%p` produces a wrong socket path if `%r` is left in.

/// The values a token can expand to for a given connection.
#[derive(Clone, Debug, Default)]
pub struct TokenContext {
    /// `%h` — the hostname after `HostName` substitution.
    pub hostname: String,
    /// `%n` — the original alias as typed.
    pub original_host: String,
    /// `%p` — port.
    pub port: u16,
    /// `%r` — remote username.
    pub remote_user: String,
    /// `%u` — local username.
    pub local_user: String,
    /// `%d` — local home directory.
    pub home: String,
    /// `%L` — short local hostname; `%l` is the full one.
    pub local_hostname: String,
}

/// Expand `%`-tokens in a config value.
///
/// Unknown tokens are left as written rather than silently deleted: a
/// `ProxyCommand` that quietly loses part of itself fails in a way nobody can
/// read, whereas one that still contains `%j` at least names the problem.
pub fn expand_tokens(value: &str, ctx: &TokenContext) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('h') => out.push_str(&ctx.hostname),
            Some('n') => out.push_str(&ctx.original_host),
            Some('p') => out.push_str(&ctx.port.to_string()),
            Some('r') => out.push_str(&ctx.remote_user),
            Some('u') => out.push_str(&ctx.local_user),
            Some('d') => out.push_str(&ctx.home),
            Some('l') => out.push_str(&ctx.local_hostname),
            Some('L') => out.push_str(
                ctx.local_hostname
                    .split('.')
                    .next()
                    .unwrap_or(&ctx.local_hostname),
            ),
            Some(other) => {
                // Preserve unknown tokens verbatim.
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> TokenContext {
        TokenContext {
            hostname: "10.0.0.5".into(),
            original_host: "prod-db".into(),
            port: 2222,
            remote_user: "deploy".into(),
            local_user: "matt".into(),
            home: "/Users/matt".into(),
            local_hostname: "laptop.local".into(),
        }
    }

    #[test]
    fn proxycommand_tokens_expand() {
        let out = expand_tokens("cloudflared access ssh --hostname %h --port %p", &ctx());
        assert_eq!(
            out,
            "cloudflared access ssh --hostname 10.0.0.5 --port 2222"
        );
    }

    #[test]
    fn controlpath_tokens_expand() {
        let out = expand_tokens("~/.ssh/cm-%r@%h:%p", &ctx());
        assert_eq!(out, "~/.ssh/cm-deploy@10.0.0.5:2222");
    }

    #[test]
    fn original_alias_and_hostname_are_different_tokens() {
        // %n is what the user typed; %h is what it resolved to. Conflating
        // them breaks ProxyCommands that key off the alias.
        let out = expand_tokens("%n -> %h", &ctx());
        assert_eq!(out, "prod-db -> 10.0.0.5");
    }

    #[test]
    fn short_and_long_local_hostnames_differ() {
        assert_eq!(expand_tokens("%L", &ctx()), "laptop");
        assert_eq!(expand_tokens("%l", &ctx()), "laptop.local");
    }

    #[test]
    fn double_percent_is_a_literal() {
        assert_eq!(expand_tokens("100%%", &ctx()), "100%");
    }

    #[test]
    fn unknown_tokens_survive_instead_of_vanishing() {
        // A ProxyCommand that silently loses a token fails unreadably.
        assert_eq!(expand_tokens("ssh -J %j %h", &ctx()), "ssh -J %j 10.0.0.5");
    }

    #[test]
    fn a_trailing_percent_is_not_a_panic() {
        assert_eq!(expand_tokens("weird%", &ctx()), "weird%");
    }

    #[test]
    fn home_expands_for_identityfile() {
        assert_eq!(
            expand_tokens("%d/.ssh/id_ed25519", &ctx()),
            "/Users/matt/.ssh/id_ed25519"
        );
    }
}
