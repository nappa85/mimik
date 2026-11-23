use crate::{AppError, Character, CharacterInput, Store};
use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    characters: Vec<Character>,
}

#[derive(Template)]
#[template(path = "editor.html")]
struct EditorTemplate<'a> {
    character: &'a Character,
    message: &'a str,
}

#[derive(Template)]
#[template(path = "character.html")]
struct CharacterTemplate<'a> {
    character: &'a Character,
    message: &'a str,
}

pub(crate) fn render(template: &impl Template) -> Result<Html<String>, AppError> {
    template.render().map(Html).map_err(|_| AppError::Template)
}

pub fn app(store: Store) -> Router {
    Router::new()
        .merge(crate::sources::web::routes())
        .route("/", get(index))
        .route("/characters/{id}", get(character).post(save))
        .route("/assets/app.css", get(css_asset))
        .route(
            "/assets/htmx.min.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../assets/htmx.min.js"),
                )
            }),
        )
        .with_state(store)
        .route("/assets/logo.png", get(logo_asset))
        .route(
            "/assets/response-targets.min.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../assets/response-targets.min.js"),
                )
            }),
        )
        .route(
            "/assets/source-review.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../assets/source-review.js"),
                )
            }),
        )
        .route(
            "/assets/source-regions.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../assets/source-regions.js"),
                )
            }),
        )
        .route(
            "/assets/account-sharing.js",
            get(crate::presentation::script_asset),
        )
        .layer(middleware::from_fn(check_authority))
}

// The listener is fixed to loopback:3000. Reject foreign authorities on reads too.
pub(crate) async fn css_asset() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../assets/app.css"),
    )
}

pub(crate) async fn logo_asset() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/png")],
        include_bytes!("../logo.png").as_slice(),
    )
}

pub(crate) async fn check_authority(request: Request, next: Next) -> Response {
    let mut hosts = request.headers().get_all(header::HOST).iter();
    let allowed = hosts
        .next()
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| {
            host.eq_ignore_ascii_case("127.0.0.1:3000")
                || host.eq_ignore_ascii_case("localhost:3000")
        });
    if !allowed || hosts.next().is_some() {
        return (
            StatusCode::BAD_REQUEST,
            "Use the local Mimik address: 127.0.0.1:3000 or localhost:3000.",
        )
            .into_response();
    }
    next.run(request).await
}

async fn index(State(store): State<Store>) -> Result<Html<String>, AppError> {
    render(&IndexTemplate {
        characters: store.list().await?,
    })
}

// Local forms must originate from this host; normal non-browser clients may omit Origin.
pub(crate) fn check_origin(headers: &HeaderMap) -> Result<(), AppError> {
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return Err(AppError::Invalid(
            "Cross-site form submissions are not accepted.",
        ));
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if origin.to_str().ok() != Some(format!("http://{host}").as_str()) {
            return Err(AppError::Invalid(
                "Form origin does not match this application.",
            ));
        }
    }
    Ok(())
}

async fn character(
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Result<Html<String>, AppError> {
    let character = store.get(id).await?;
    render(&CharacterTemplate {
        character: &character,
        message: "",
    })
}

async fn save(
    State(store): State<Store>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(input): Form<CharacterInput>,
) -> Result<Response, AppError> {
    check_origin(&headers)?;
    let is_htmx = headers.get("hx-request").is_some_and(|v| v == "true");
    match store.update(id, input.clone()).await {
        Ok(character) if is_htmx => Ok(render(&EditorTemplate {
            character: &character,
            message: "Saved on this device.",
        })?
        .into_response()),
        Ok(_) => Ok(Redirect::to(&format!("/characters/{id}")).into_response()),
        Err(error @ (AppError::Invalid(_) | AppError::Conflict)) => {
            // Preserve the submitted values and revision, especially on stale-tab conflicts.
            let mut character = store.get(id).await?;
            character.input = input;
            let message = error.to_string();
            if is_htmx {
                // htmx swaps successful responses by default; errors are displayed in the form.
                Ok(render(&EditorTemplate {
                    character: &character,
                    message: &message,
                })?
                .into_response())
            } else {
                Ok((
                    error.status(),
                    render(&CharacterTemplate {
                        character: &character,
                        message: &message,
                    })?,
                )
                    .into_response())
            }
        }
        Err(error) => Err(error),
    }
}

impl AppError {
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound | Self::SourceNotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let message = if self.status() == StatusCode::INTERNAL_SERVER_ERROR
            && !matches!(self, Self::PdfCorrupt)
        {
            eprintln!("Mimik error: {self}");
            "An internal error occurred. Check the application log.".into()
        } else {
            self.to_string()
        };
        (self.status(), message).into_response()
    }
}
