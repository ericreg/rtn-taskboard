use anyhow::ensure;
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// The browser-facing origin of this gateway, independent of backend link settings.
#[derive(Clone)]
pub struct BrowserOrigin(String);

impl BrowserOrigin {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let url = url::Url::parse(value)?;
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.has_host()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "expected an http(s) origin without credentials, path, query, or fragment"
        );
        Ok(Self(url.origin().ascii_serialization()))
    }

    fn allows(&self, request: &Request) -> bool {
        if matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        ) {
            return true;
        }
        let headers = request.headers();
        // Check metadata even when Origin is absent. Do not infer the allowed
        // origin from untrusted Host, Forwarded, or X-Forwarded-* headers.
        if headers
            .get_all("sec-fetch-site")
            .iter()
            .any(|h| h == "cross-site")
        {
            return false;
        }
        let mut origins = headers.get_all(header::ORIGIN).iter();
        match origins.next() {
            // Preserve non-browser clients; backend session/CSRF checks still apply.
            None => true,
            Some(origin) => {
                origins.next().is_none() && origin.to_str().is_ok_and(|value| value == self.0)
            }
        }
    }
}

pub async fn protect(
    State(origin): State<BrowserOrigin>,
    request: Request,
    next: Next,
) -> Response {
    if !origin.allows(&request) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":{"code":"forbidden","message":"Browser origin rejected by gateway. Open the app at its configured TASKBOARD_GATEWAY_ORIGIN."}}"#,
        ).into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, middleware, routing::any};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn configuration_requires_an_http_origin() {
        assert_eq!(
            BrowserOrigin::parse("https://GATEWAY.test:443/").unwrap().0,
            "https://gateway.test"
        );
        assert_eq!(
            BrowserOrigin::parse("http://[::1]:8080").unwrap().0,
            "http://[::1]:8080"
        );
        for value in [
            "",
            "null",
            "*",
            "file:///tmp",
            "ftp://gateway.test",
            "https://user:pass@gateway.test",
            "https://gateway.test/path",
            "https://gateway.test?query",
            "https://gateway.test#fragment",
        ] {
            assert!(BrowserOrigin::parse(value).is_err(), "{value}");
        }
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/api/v1/auth/login",
                any(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(middleware::from_fn_with_state(
                BrowserOrigin::parse("https://gateway.test").unwrap(),
                protect,
            ))
    }

    #[tokio::test]
    async fn checks_browser_writes_before_the_handler() {
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            for (origin, site, allowed) in [
                (Some("https://gateway.test"), Some("same-origin"), true),
                (Some("https://gateway.test"), None, true),
                (None, None, true),
                (None, Some("same-origin"), true),
                (None, Some("cross-site"), false),
                (Some("https://gateway.test"), Some("cross-site"), false),
                (Some("https://other.test"), Some("same-origin"), false),
                (Some("https://other.test"), Some("same-site"), false),
                (Some("https://other.test"), None, false),
                (Some("http://gateway.test"), None, false),
                (Some("https://gateway.test:8080"), None, false),
                (Some("https://gateway.test.attacker.test"), None, false),
                (Some("https://gateway.test/path"), None, false),
                (Some("null"), None, false),
                (Some(""), None, false),
                (Some("https://gateway.test https://other.test"), None, false),
            ] {
                let mut request = Request::builder().method(method).uri("/api/v1/auth/login");
                if let Some(origin) = origin {
                    request = request.header(header::ORIGIN, origin);
                }
                if let Some(site) = site {
                    request = request.header("sec-fetch-site", site);
                }
                let response = app()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    if allowed {
                        StatusCode::NO_CONTENT
                    } else {
                        StatusCode::FORBIDDEN
                    },
                    "{method} {origin:?} {site:?}"
                );
                if !allowed {
                    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
                    let body = response.into_body().collect().await.unwrap().to_bytes();
                    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(error["error"]["code"], "forbidden");
                }
            }
        }
    }

    #[tokio::test]
    async fn safe_requests_still_reach_the_handler() {
        for method in ["GET", "HEAD", "OPTIONS"] {
            let request = Request::builder()
                .method(method)
                .uri("/api/v1/auth/login")
                .header(header::ORIGIN, "https://other.test")
                .header("sec-fetch-site", "cross-site")
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app().oneshot(request).await.unwrap().status(),
                StatusCode::NO_CONTENT
            );
        }
    }

    #[tokio::test]
    async fn duplicate_origins_and_spoofed_proxy_headers_cannot_bypass_checks() {
        for request in [
            Request::builder()
                .header(header::ORIGIN, "https://gateway.test")
                .header(header::ORIGIN, "https://other.test"),
            Request::builder()
                .header(header::ORIGIN, "https://other.test")
                .header(header::HOST, "other.test")
                .header("forwarded", "host=other.test;proto=https")
                .header("x-forwarded-host", "other.test")
                .header("x-forwarded-proto", "https"),
        ] {
            let request = request
                .method("POST")
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app().oneshot(request).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
    }
}
