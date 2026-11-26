//! [dot-dot] A request whose path holds `..` is refused, 400, before any
//! route, in front of both routers (single-user `main.rs`, multi-user
//! `multiuser.rs`). Checked on the path as it came off the wire: hyper does
//! not resolve dot segments, and an `%xx` escape is decoded before the check,
//! twice over, for an escape escaped again. The query is not the path.

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::Router;

/// [dot-dot] Does a request's path hold `..` - as sent, or once its `%xx`
/// escapes are decoded (twice over, for an escape escaped again)?
pub fn path_has_dot_dot(path: &str) -> bool {
    let mut p = path.to_string();
    for _ in 0..3 {
        if p.contains("..") {
            return true;
        }
        let decoded = percent_decoded(&p);
        if decoded == p {
            return false;
        }
        p = decoded;
    }
    p.contains("..")
}

/// `%xx` escapes decoded, the rest as it is; an escape that is no hex pair
/// stays as written.
fn percent_decoded(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn reject_dot_dot(req: Request, next: Next) -> Response {
    if path_has_dot_dot(req.uri().path()) {
        let mut resp = Response::new(Body::from("bad path"));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return resp;
    }
    next.run(req).await
}

/// The router with what goes around every route of it.
pub fn guarded(app: Router) -> Router {
    app.layer(axum::middleware::from_fn(reject_dot_dot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tower::ServiceExt;

    /// [dot-dot] `..` in the path, as sent or escaped, is found; in the
    /// query, or a single dot, not.
    #[test]
    fn a_path_with_dot_dot_is_found_as_sent_and_escaped() {
        for p in [
            "/api/../x",
            "/..",
            "/a..b",
            "/%2e%2e/x",
            "/.%2E/x",
            "/x/%252e%252e/y",
        ] {
            assert!(path_has_dot_dot(p), "{p}");
        }
        for p in ["/", "/servers", "/a.b/c.d", "/%2e/x", "/%"] {
            assert!(!path_has_dot_dot(p), "{p}");
        }
    }

    /// [dot-dot] such a path is refused with 400 before any route; a plain
    /// one is served, and `..` in the query is left to the handler.
    #[tokio::test]
    async fn the_router_refuses_a_path_with_dot_dot() {
        let app = guarded(Router::new().route("/servers", get(|| async { "ok" })));
        let get_ = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();
        for uri in ["/x/../servers", "/%2e%2e/servers", "/api/..%2Fservers"] {
            let r = app.clone().oneshot(get_(uri)).await.unwrap();
            assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{uri}");
        }
        for uri in ["/servers", "/servers?next=../x"] {
            let r = app.clone().oneshot(get_(uri)).await.unwrap();
            assert_eq!(r.status(), StatusCode::OK, "{uri}");
        }
    }
}
