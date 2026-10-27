//! Public-only, DNS-pinned outbound HTTP transport.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

pub(crate) fn strip_ipv6_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host)
}

fn forbidden_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || (a == 198 && (b == 18 || b == 19))
}

pub(crate) fn is_forbidden_target(ip: IpAddr) -> bool {
    if let IpAddr::V4(ip) = ip.to_canonical() {
        return forbidden_v4(ip);
    }
    let IpAddr::V6(ip) = ip else { unreachable!() };
    let s = ip.segments();
    if (s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|v| *v == 0))
        || s[..6].iter().all(|v| *v == 0)
    {
        return forbidden_v4(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }
    // Only global unicast; exclude local-use NAT64 and transition/metadata ranges.
    s[0] & 0xe000 != 0x2000
        || s[0] == 0x2002
        || (s[0] == 0x2001 && s[1] == 0xdb8)
        || (s[0] == 0x3fff && s[1] < 0x1000)
        || (s[0] == 0x2001 && s[1] < 0x200)
        || ip.is_unicast_link_local()
        || ip.is_unique_local()
}

pub(crate) async fn resolve_vetted_addrs(url: &reqwest::Url) -> Option<Vec<SocketAddr>> {
    let host = strip_ipv6_brackets(url.host_str()?);
    let port = url.port_or_known_default()?;
    let addrs: Vec<_> = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .ok()?
    .ok()?
    .collect();
    vetted_addrs(addrs)
}

fn vetted_addrs(addrs: Vec<SocketAddr>) -> Option<Vec<SocketAddr>> {
    if addrs.is_empty() || addrs.iter().any(|a| is_forbidden_target(a.ip())) {
        None
    } else {
        Some(addrs)
    }
}

pub(crate) fn build_pinned_client(
    host: &str,
    addrs: &[SocketAddr],
) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(strip_ipv6_brackets(host), addrs)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_resolution_rejects_mixed_and_embedded_private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "2002:0a00:0001::1",
            "fc00::1",
        ] {
            assert!(is_forbidden_target(ip.parse().unwrap()), "{ip}");
        }
        let public = "1.1.1.1:443".parse().unwrap();
        assert!(vetted_addrs(vec![public]).is_some());
        assert!(vetted_addrs(vec![public, "127.0.0.1:443".parse().unwrap()]).is_none());
    }

    #[test]
    fn document_public_policy_excludes_reserved_literals_mapped_and_mixed_dns() {
        for ip in [
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "198.51.100.1",
            "203.0.113.1",
            "::ffff:198.51.100.1",
            "::ffff:203.0.113.1",
            "64:ff9b::c633:6401",
            "2001:db8::1",
            "3fff::1",
            "3fff:fff:ffff::1",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(is_forbidden_target(ip), "{ip}");
            if let IpAddr::V4(v4) = ip {
                let mapped = IpAddr::V6(v4.to_ipv6_mapped());
                assert!(is_forbidden_target(mapped), "mapped {ip}");
                assert!(vetted_addrs(vec![
                    "1.1.1.1:443".parse().unwrap(),
                    SocketAddr::new(mapped, 443)
                ])
                .is_none());
            }
            assert!(
                vetted_addrs(vec![SocketAddr::new(ip, 443)]).is_none(),
                "{ip}"
            );
            assert!(
                vetted_addrs(vec![
                    "1.1.1.1:443".parse().unwrap(),
                    SocketAddr::new(ip, 443)
                ])
                .is_none(),
                "mixed DNS {ip}"
            );
        }
        for ip in [
            "1.1.1.1",
            "192.0.66.1",
            "192.0.78.1",
            "192.88.98.1",
            "198.51.99.1",
            "203.0.114.1",
            "2606:4700:4700::1111",
            "3fff:1000::1",
            "::ffff:192.0.78.1",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!is_forbidden_target(ip), "public {ip}");
            assert!(vetted_addrs(vec![SocketAddr::new(ip, 443)]).is_some());
        }
    }

    #[tokio::test]
    async fn low_level_fixture_is_pinned_and_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://never-resolve.invalid/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        // Transport fixture bypasses address policy, never the public fetch entry point.
        let client = build_pinned_client("pinned.invalid", &[address]).unwrap();
        let response = client
            .get(format!("http://pinned.invalid:{}/", address.port()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 302);
        server.await.unwrap();
        assert!(
            resolve_vetted_addrs(&reqwest::Url::parse(&format!("http://{address}/")).unwrap())
                .await
                .is_none()
        );
    }
}
