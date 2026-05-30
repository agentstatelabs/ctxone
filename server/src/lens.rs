//! Embedded Lens static file server.
//!
//! Lens is built as a pure static SPA (adapter-static) and embedded
//! into the Hub binary at compile time. When the Hub is started with
//! `--lens`, these files are served alongside the `/api/` routes so
//! no separate Node.js process or deployment is needed.
//!
//! Routing: `/api/*` → Hub REST handlers (defined in http.rs).
//!          `/_app/*` → Lens immutable assets (long-cache).
//!          `/*`      → Lens SPA fallback (index.html).

use axum::{
    body::Body,
    http::{HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../web/build/"]
struct LensAssets;

/// Handler for all non-API routes — serves embedded Lens static files.
/// Unknown paths fall back to `index.html` for SPA client-side routing.
pub async fn lens_handler(req: Request<Body>) -> Response {
    let raw_path = req.uri().path();

    // Unknown /api/* routes must return JSON 404 — falling back to
    // the SPA's index.html breaks JSON.parse on the client.
    if raw_path.starts_with("/api/") {
        let body = format!(r#"{{"error":"not found","path":"{raw_path}"}}"#);
        let mut resp = Response::new(Body::from(body));
        *resp.status_mut() = StatusCode::NOT_FOUND;
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        return resp;
    }

    let path = raw_path.trim_start_matches('/');

    // Try exact path first, then path/index.html for directory routes,
    // then fall back to root index.html for SPA routing.
    let asset = LensAssets::get(path)
        .or_else(|| {
            let with_index = format!("{}/index.html", path.trim_end_matches('/'));
            LensAssets::get(&with_index)
        })
        .or_else(|| LensAssets::get("index.html"));

    match asset {
        Some(file) => {
            let mime = file.metadata.mimetype();
            let body = Body::from(file.data.into_owned());
            let mut resp = Response::new(body);

            resp.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(mime)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            );

            // Immutable assets (content-hashed filenames) get long cache.
            // Everything else (index.html, route pages) gets no-cache so
            // updates are picked up immediately on Hub restart.
            if path.starts_with("_app/immutable/") {
                resp.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                );
            } else {
                resp.headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            }

            resp
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
