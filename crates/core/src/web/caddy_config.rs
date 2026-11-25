//! Caddy JSON configuration of the local sites.
//!
//! Shape of the generated configuration:
//!
//! - the admin API is disabled (no TCP endpoint that other local users
//!   could reach) and the configuration is never persisted by Caddy: the
//!   daemon restarts Caddy to apply changes;
//! - an HTTPS server on the loopback address of the sites with one route
//!   per served site (`<name>.<tld>` and `*.<name>.<tld>`), then a 404
//!   fallback;
//! - an HTTP server on the same address redirecting every request to
//!   HTTPS on the standard port (reached through the port forwarding);
//! - certificates issued by the local certificate authority `local`, whose
//!   trust is never installed by Caddy itself (that is the privileged
//!   helper's job, after approval).

use std::path::Path;

use serde_json::{Value, json};

use super::site::{Site, SiteTarget};
use crate::config::WebConfig;

/// Name of the local certificate authority, used in its subject.
pub const CA_NAME: &str = "DevSpace Local CA";

/// Response header added to the HTTP redirects, used to recognise the
/// DevSpace web server behind the port forwarding.
pub const PROBE_HEADER: &str = "X-DevSpace";

/// Time given to open connections when Caddy stops.
const GRACE_PERIOD: &str = "2s";

/// Renders the configuration serving `sites` (unavailable sites answer 503
/// with the reason) on the address and ports of `web`, with certificates in
/// `storage_dir`.
#[must_use]
pub fn render(sites: &[Site], web: &WebConfig, storage_dir: &Path) -> Value {
    let mut routes = Vec::new();
    let mut subjects = Vec::new();
    for site in sites {
        let handler = match &site.target {
            SiteTarget::Static { root } => json!({"handler": "file_server", "root": root}),
            SiteTarget::Php { root, socket, .. } => php_handler(root, socket),
            SiteTarget::Proxy { upstream } => json!({
                "handler": "reverse_proxy",
                "upstreams": [{"dial": upstream.as_str()}],
            }),
            // Still routed, so that the browser explains instead of
            // failing the TLS handshake. Braces would be Caddy placeholders.
            SiteTarget::Unavailable { reason } => json!({
                "handler": "static_response",
                "status_code": 503,
                "headers": {"Content-Type": ["text/plain; charset=utf-8"]},
                "body": format!("DevSpace: {} is not served.\n\n{reason}\n", site.domain)
                    .replace(['{', '}'], ""),
            }),
        };
        let hosts = [site.domain.clone(), format!("*.{}", site.domain)];
        routes.push(json!({
            "match": [{"host": hosts}],
            "handle": [handler],
            "terminal": true,
        }));
        subjects.extend(hosts);
    }
    routes.push(json!({
        "handle": [{
            "handler": "static_response",
            "status_code": 404,
            "body": "No DevSpace project is served at this address.\n",
        }],
    }));

    let mut apps = json!({
        "http": {
            "http_port": web.http_port,
            "https_port": web.https_port,
            "grace_period": GRACE_PERIOD,
            "servers": {
                "devspace_https": {
                    "listen": [format!("{}:{}", web.address, web.https_port)],
                    "routes": routes,
                    // Error responses get a short explanation instead of
                    // an empty page.
                    "errors": {"routes": [{
                        "handle": [{
                            "handler": "static_response",
                            "status_code": "{http.error.status_code}",
                            "body": "DevSpace: {http.error.status_code} {http.error.status_text}\n",
                        }],
                    }]},
                    "automatic_https": {"disable_redirects": true},
                },
                "devspace_http": {
                    "listen": [format!("{}:{}", web.address, web.http_port)],
                    "routes": [{
                        "handle": [{
                            "handler": "static_response",
                            "status_code": 308,
                            "headers": {
                                "Location": ["https://{http.request.host}{http.request.uri}"],
                                PROBE_HEADER: ["1"],
                            },
                        }],
                    }],
                    "automatic_https": {"disable": true},
                },
            },
        },
        "pki": {
            "certificate_authorities": {
                "local": {"name": CA_NAME, "install_trust": false},
            },
        },
    });
    if !subjects.is_empty() {
        apps["tls"] = json!({
            "automation": {
                "policies": [{
                    "subjects": subjects,
                    "issuers": [{"module": "internal", "ca": "local"}],
                }],
            },
        });
    }
    json!({
        "admin": {"disabled": true, "config": {"persist": false}},
        "storage": {"module": "file_system", "root": storage_dir},
        "apps": apps,
    })
}

/// Equivalent of the `php_fastcgi` directive of the Caddyfile (output of
/// `caddy adapt` for Caddy 2.11): directories get a trailing slash, paths
/// that are not files go to `index.php`, `.php` scripts are executed by
/// PHP-FPM through its Unix socket, everything else is a static file.
fn php_handler(root: &Path, socket: &Path) -> Value {
    json!({
        "handler": "subroute",
        "routes": [
            {"handle": [{"handler": "vars", "root": root}]},
            {
                "match": [{
                    "file": {"try_files": ["{http.request.uri.path}/index.php"]},
                    "not": [{"path": ["*/"]}],
                }],
                "handle": [{
                    "handler": "static_response",
                    "status_code": 308,
                    "headers": {"Location": [
                        "./{http.request.orig_uri.path.file_escaped}/{http.request.orig_uri.prefixed_query}",
                    ]},
                }],
            },
            {
                "match": [{"file": {
                    "try_files": [
                        "{http.request.uri.path}",
                        "{http.request.uri.path}/index.php",
                        "index.php",
                    ],
                    "try_policy": "first_exist_fallback",
                    "split_path": [".php"],
                }}],
                "handle": [{"handler": "rewrite", "uri": "{http.matchers.file.relative}"}],
            },
            {
                "match": [{"path": ["*.php"]}],
                "handle": [{
                    "handler": "reverse_proxy",
                    "transport": {"protocol": "fastcgi", "split_path": [".php"]},
                    "upstreams": [{"dial": format!("unix/{}", socket.display())}],
                }],
            },
            {"handle": [{"handler": "file_server"}]},
        ],
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::project::ProjectName;
    use crate::project::manifest::ProxyUpstream;

    fn site(name: &str, target: SiteTarget) -> Site {
        let project = ProjectName::parse(name).unwrap();
        Site {
            domain: super::super::site::domain(&project, &WebConfig::default().tld),
            project,
            target,
        }
    }

    fn sample() -> Value {
        render(
            &[
                site(
                    "blog",
                    SiteTarget::Static {
                        root: PathBuf::from("/src/blog/public"),
                    },
                ),
                site(
                    "api",
                    SiteTarget::Proxy {
                        upstream: ProxyUpstream::parse("127.0.0.1:3000").unwrap(),
                    },
                ),
                site(
                    "shop",
                    SiteTarget::Php {
                        root: PathBuf::from("/src/shop/public"),
                        version: crate::runtime::Version::parse("8.4.23").unwrap(),
                        socket: PathBuf::from("/run/php/8.4.23.sock"),
                    },
                ),
                site(
                    "broken",
                    SiteTarget::Unavailable {
                        reason: "no public directory".into(),
                    },
                ),
            ],
            &WebConfig::default(),
            Path::new("/data/caddy/storage"),
        )
    }

    #[test]
    fn admin_api_is_disabled_and_only_the_sites_address_listens() {
        let config = sample();
        assert_eq!(config["admin"]["disabled"], json!(true));
        assert_eq!(config["admin"]["config"]["persist"], json!(false));
        let servers = config["apps"]["http"]["servers"].as_object().unwrap();
        for server in servers.values() {
            for address in server["listen"].as_array().unwrap() {
                let address = address.as_str().unwrap();
                assert!(address.starts_with("127.0.0.2:"), "{address}");
            }
        }
        assert_eq!(
            config["apps"]["pki"]["certificate_authorities"]["local"]["install_trust"],
            json!(false)
        );
        assert_eq!(config["storage"]["root"], json!("/data/caddy/storage"));
    }

    #[test]
    fn served_sites_get_routes_and_certificates() {
        let config = sample();
        let routes = config["apps"]["http"]["servers"]["devspace_https"]["routes"]
            .as_array()
            .unwrap();
        assert_eq!(routes.len(), 5, "four sites and the fallback");
        assert_eq!(
            routes[0]["match"][0]["host"],
            json!(["blog.internal", "*.blog.internal"])
        );
        assert_eq!(
            routes[0]["handle"][0],
            json!({"handler": "file_server", "root": "/src/blog/public"})
        );
        assert_eq!(
            routes[1]["handle"][0]["upstreams"],
            json!([{"dial": "127.0.0.1:3000"}])
        );
        let php = &routes[2]["handle"][0];
        assert_eq!(php["handler"], json!("subroute"));
        assert_eq!(
            php["routes"][0]["handle"][0],
            json!({"handler": "vars", "root": "/src/shop/public"})
        );
        assert_eq!(
            php["routes"][3]["handle"][0]["upstreams"],
            json!([{"dial": "unix//run/php/8.4.23.sock"}])
        );
        assert_eq!(
            php["routes"][3]["handle"][0]["transport"]["protocol"],
            json!("fastcgi")
        );
        assert_eq!(routes[3]["handle"][0]["status_code"], json!(503));
        let body = routes[3]["handle"][0]["body"].as_str().unwrap();
        assert!(body.contains("broken.internal is not served"), "{body}");
        assert!(body.contains("no public directory"), "{body}");
        assert_eq!(routes[4]["handle"][0]["status_code"], json!(404));
        assert_eq!(
            config["apps"]["tls"]["automation"]["policies"][0]["subjects"],
            json!([
                "blog.internal",
                "*.blog.internal",
                "api.internal",
                "*.api.internal",
                "shop.internal",
                "*.shop.internal",
                "broken.internal",
                "*.broken.internal"
            ])
        );
    }

    #[test]
    fn http_redirects_to_the_standard_https_port() {
        let config = sample();
        let server = &config["apps"]["http"]["servers"]["devspace_http"];
        assert_eq!(server["listen"], json!(["127.0.0.2:47080"]));
        let headers = &server["routes"][0]["handle"][0]["headers"];
        assert_eq!(
            headers["Location"],
            json!(["https://{http.request.host}{http.request.uri}"])
        );
        assert_eq!(headers[PROBE_HEADER], json!(["1"]));
    }

    #[test]
    fn no_tls_policy_without_sites() {
        let config = render(&[], &WebConfig::default(), Path::new("/storage"));
        assert!(config["apps"].get("tls").is_none());
        assert!(config["apps"]["pki"].is_object(), "the CA is still created");
    }
}
