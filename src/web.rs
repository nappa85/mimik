use crate::games::{CreateRequest, CreationCatalog};
use crate::mork_borg::Sheet;
use crate::mork_form::{SheetDraft, SheetForm};
use crate::{AppError, Character, CharacterInput, Store};
use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    characters: Vec<Character>,
    catalog: CreationCatalog,
    draft: CreateRequest,
    message: String,
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

#[derive(Template)]
#[template(path = "mork_borg.html")]
struct MorkTemplate<'a> {
    character: &'a Character,
    sheet: &'a Sheet,
    draft: &'a SheetDraft,
    message: &'a str,
    saved: bool,
}

#[derive(Template)]
#[template(path = "mork_editor.html")]
struct MorkEditorTemplate<'a> {
    character: &'a Character,
    sheet: &'a Sheet,
    draft: &'a SheetDraft,
    message: &'a str,
    saved: bool,
}

pub(crate) fn render(template: &impl Template) -> Result<Html<String>, AppError> {
    template.render().map(Html).map_err(|_| AppError::Template)
}

pub fn app(store: Store) -> Router {
    Router::new()
        .merge(crate::sources::web::routes())
        .route("/", get(index))
        .route("/characters", post(create))
        .route("/characters/{id}", get(character).post(save))
        .route(
            "/characters/{id}/mork-borg",
            get(mork_character).post(mork_save),
        )
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
        catalog: store.creation_catalog().await?,
        draft: CreateRequest::default(),
        message: String::new(),
    })
}

async fn mork_character(
    State(store): State<Store>,
    Path(id): Path<i64>,
) -> Result<Html<String>, AppError> {
    let (character, sheet, saved) = store.mork_borg_sheet(id).await?;
    let draft = SheetDraft::saved(&character, &sheet);
    render(&MorkTemplate {
        character: &character,
        sheet: &sheet,
        draft: &draft,
        message: "",
        saved,
    })
}

async fn mork_save(
    State(store): State<Store>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    submission: Result<Form<SheetForm>, axum::extract::rejection::FormRejection>,
) -> Result<Response, AppError> {
    let htmx = headers.get("hx-request").is_some_and(|v| v == "true");
    if check_origin(&headers).is_err() {
        return Ok(sheet_transport_error(
            StatusCode::FORBIDDEN,
            if htmx {
                "Form origin rejected. Your displayed draft was kept; use the local application address."
            } else {
                "Form origin rejected. No changes were saved; use the local application address."
            },
        ));
    }
    let form = match submission {
        Ok(Form(form)) => form,
        Err(error) => {
            return Ok(sheet_transport_error(
                error.status(),
                if htmx {
                    "Submission format invalid: remove unsupported or repeated fields and retry. Your displayed draft was kept."
                } else {
                    "Submission format invalid: remove unsupported or repeated fields and retry. No changes were saved."
                },
            ));
        }
    };
    let (current_character, current_sheet, _) = store.mork_borg_sheet(id).await?;
    let draft = SheetDraft::submitted(&form, &current_character, &current_sheet);
    let result = match draft.validate() {
        Ok(sheet) => {
            store
                .save_mork_borg_sheet_named(id, sheet, form.name.clone())
                .await
        }
        Err(error) => Err(error),
    };
    match result {
        Ok((character, sheet)) if htmx => Ok(render(&MorkEditorTemplate {
            character: &character,
            sheet: &sheet,
            draft: &SheetDraft::saved(&character, &sheet),
            message: "Sheet saved. Derived values updated.",
            saved: true,
        })?
        .into_response()),
        Ok(_) => Ok(Redirect::to(&format!("/characters/{id}/mork-borg")).into_response()),
        Err(error @ (AppError::Invalid(_) | AppError::Conflict)) => {
            let message = error.to_string();
            if htmx {
                Ok((
                    error.status(),
                    render(&MorkEditorTemplate {
                        character: &current_character,
                        sheet: &current_sheet,
                        draft: &draft,
                        message: &message,
                        saved: false,
                    })?,
                )
                    .into_response())
            } else {
                Ok((
                    error.status(),
                    render(&MorkTemplate {
                        character: &current_character,
                        sheet: &current_sheet,
                        draft: &draft,
                        message: &message,
                        saved: false,
                    })?,
                )
                    .into_response())
            }
        }
        Err(error) => Err(error),
    }
}

fn sheet_transport_error(status: StatusCode, message: &'static str) -> Response {
    (
        status,
        [("HX-Retarget", "#mork-message"), ("HX-Reswap", "innerHTML")],
        Html(message),
    )
        .into_response()
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

async fn create(
    State(store): State<Store>,
    headers: HeaderMap,
    Form(input): Form<CreateRequest>,
) -> Result<Response, AppError> {
    check_origin(&headers)?;
    match store.create_available_character(input.clone()).await {
        Ok(result) => {
            Ok(Redirect::to(&format!("/characters/{}/mork-borg", result.id)).into_response())
        }
        Err(error @ AppError::Invalid(_)) => Ok((
            error.status(),
            render(&IndexTemplate {
                characters: store.list().await?,
                catalog: store.creation_catalog().await?,
                draft: input,
                message: error.to_string(),
            })?,
        )
            .into_response()),
        Err(error) => Err(error),
    }
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
