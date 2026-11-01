use super::*;
use crate::sources::{
    uploads::{Uploads, Worker},
    worker,
};
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream;

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

struct Refuse;
struct Unavailable;
impl Worker for Unavailable {
    fn availability(&self) -> worker::Availability {
        worker::Availability::Unavailable {
            reason: "Fixed probe reason".into(),
            how_to_fix: "Fixed probe remedy".into(),
        }
    }
    fn inspect(
        &self,
        _: &std::path::Path,
        _: &worker::Cancel,
    ) -> Result<worker::Inspection, worker::Reason> {
        unreachable!()
    }
    fn render(
        &self,
        _: &std::path::Path,
        _: &[worker::Page],
        _: &worker::Cancel,
        _: &mut (dyn FnMut(worker::RenderedPage) -> Result<(), worker::Error> + Send),
    ) -> Result<(), worker::Reason> {
        unreachable!()
    }
}
#[tokio::test]
async fn upload_route_unavailable_polls_no_body_and_shows_probe_remedy() {
    let (dir, _, app, id, m) = setup_worker(Arc::new(Unavailable)).await;
    let polls = Arc::new(AtomicUsize::new(0));
    let seen = polls.clone();
    let body = Body::from_stream(stream::poll_fn(move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
        std::task::Poll::Ready(None::<Result<Bytes, std::io::Error>>)
    }));
    let response = app.oneshot(post_body(id, body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(polls.load(Ordering::Relaxed), 0);
    let text = body_text(response).await;
    assert!(text.contains("Fixed probe reason"));
    assert!(text.contains("Fixed probe remedy"));
    clean(&m, &dir).await;
}
impl Worker for Refuse {
    fn availability(&self) -> worker::Availability {
        worker::Availability::Available
    }
    fn inspect(
        &self,
        _: &std::path::Path,
        _: &worker::Cancel,
    ) -> Result<worker::Inspection, worker::Reason> {
        Err(worker::Reason::Document(worker::Error::Unsupported))
    }
    fn render(
        &self,
        _: &std::path::Path,
        _: &[worker::Page],
        _: &worker::Cancel,
        _: &mut (dyn FnMut(worker::RenderedPage) -> Result<(), worker::Error> + Send),
    ) -> Result<(), worker::Reason> {
        unreachable!()
    }
}
async fn setup() -> (TestDirectory, Store, Router, Id, Uploads) {
    setup_worker(Arc::new(Refuse)).await
}
async fn setup_worker(worker: Arc<dyn Worker>) -> (TestDirectory, Store, Router, Id, Uploads) {
    let (dir, store, sessions, clock) = fixture().await;
    store.run(|db| {
        db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'owner','Owner','enabled',1,?1)", ["$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;
        db.execute("INSERT INTO account_owner VALUES(1,1)", [])?;
        Ok(())
    }).await.unwrap();
    let mut r = record(clock.load(Ordering::SeqCst));
    r.data
        .insert("principal".into(), serde_json::json!({"id":1,"epoch":1}));
    sessions.create(&mut r).await.unwrap();
    let m = Uploads::for_route_test(store.clone(), dir.path(), worker);
    let mut s = state(store.clone());
    s.options.uploads = Some(m.clone());
    (dir, store, router(s), r.id, m)
}
fn multipart(fields: &[(&str, &str)], close: bool) -> String {
    let mut s = String::new();
    for (name, value) in fields {
        use std::fmt::Write;
        write!(s, "--test\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"BUYER-MARKER-NAME.pdf\"\r\n\r\n{value}\r\n").unwrap();
    }
    if close {
        s.push_str("--test--\r\n");
    }
    s
}
fn post_body(id: Id, body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/admin/materials/uploads")
        .header("host", "127.0.0.1:3000")
        .header("origin", "http://127.0.0.1:3000")
        .header("cookie", format!("mimik_session={id}"))
        .header("content-type", "multipart/form-data; boundary=test")
        .body(body)
        .unwrap()
}
async fn clean(m: &Uploads, dir: &TestDirectory) {
    for _ in 0..100 {
        let manager = m.clone();
        if tokio::task::spawn_blocking(move || manager.begin_upload().is_ok())
            .await
            .unwrap()
        {
            assert_eq!(
                std::fs::read_dir(dir.path().join("staging"))
                    .unwrap()
                    .count(),
                0
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("upload slot leaked");
}
#[tokio::test]
async fn upload_route_admission_polls_no_body() {
    let (dir, _, app, id, m) = setup().await;
    for case in ["auth", "host", "origin", "length"] {
        let polls = Arc::new(AtomicUsize::new(0));
        let p = polls.clone();
        let body = Body::from_stream(stream::poll_fn(move |_| {
            p.fetch_add(1, Ordering::SeqCst);
            std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(Bytes::from_static(
                b"unexpected",
            ))))
        }));
        let mut req = post_body(id, body);
        let expected = match case {
            "auth" => {
                req.headers_mut().remove("cookie");
                StatusCode::UNAUTHORIZED
            }
            "host" => {
                req.headers_mut()
                    .insert("host", "evil.invalid".parse().unwrap());
                StatusCode::BAD_REQUEST
            }
            "origin" => {
                req.headers_mut()
                    .insert("origin", "http://evil.invalid".parse().unwrap());
                StatusCode::FORBIDDEN
            }
            _ => {
                req.headers_mut()
                    .insert("content-length", "134283265".parse().unwrap());
                StatusCode::PAYLOAD_TOO_LARGE
            }
        };
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            expected,
            "{case}"
        );
        assert_eq!(polls.load(Ordering::SeqCst), 0, "{case}");
    }
    clean(&m, &dir).await;
}
#[tokio::test]
async fn upload_route_refused_fields_and_csrf_cleanup() {
    let (dir, _, app, id, m) = setup().await;
    let token = "a".repeat(64);
    for fields in [
        vec![("csrf", "wrong"), ("file", "%PDF-1.7")],
        vec![("system", "mork-borg"), ("csrf", token.as_str())],
        vec![("csrf", token.as_str()), ("csrf", token.as_str())],
        vec![("csrf", token.as_str()), ("other", "unexpected")],
        vec![
            ("csrf", token.as_str()),
            ("system", "mork-borg"),
            ("language", "en"),
            ("edition", "Synthetic"),
        ],
        vec![
            ("csrf", token.as_str()),
            ("system", "mork-borg"),
            ("edition", "Synthetic"),
            ("language", "en"),
            ("file", "%PDF-1.7"),
            ("other", "after"),
        ],
    ] {
        let expected = if fields[0] == ("csrf", "wrong") {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
        let response = app
            .clone()
            .oneshot(post_body(id, Body::from(multipart(&fields, true))))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{fields:?}");
        clean(&m, &dir).await;
    }
}
#[tokio::test]
async fn upload_route_disconnect_drop_busy_and_responsive() {
    for disconnect in [true, false] {
        let (dir, _, app, id, m) = setup().await;
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
        let token = "a".repeat(64);
        let prefix = multipart(
            &[
                ("csrf", &token),
                ("system", "mork-borg"),
                ("edition", "Synthetic"),
                ("language", "en"),
            ],
            false,
        ) + "--test\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n%PDF-1.7\npartial";
        sender.send(Ok(Bytes::from(prefix))).await.unwrap();
        let body = Body::from_stream(stream::unfold(receiver, |mut r| async move {
            r.recv().await.map(|v| (v, r))
        }));
        let task = tokio::spawn(app.clone().oneshot(post_body(id, body)));
        let mut written = false;
        for _ in 0..100 {
            written = std::fs::read_dir(dir.path().join("staging"))
                .unwrap()
                .any(|e| e.unwrap().metadata().unwrap().len() > 0);
            if written {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(written);
        let second = app
            .clone()
            .oneshot(post_body(id, Body::empty()))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::CONFLICT);
        assert_eq!(
            app.clone()
                .oneshot(request("/auth/session", Some(id)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        if disconnect {
            sender
                .send(Err(std::io::Error::other("disconnected")))
                .await
                .unwrap();
            assert_eq!(
                task.await.unwrap().unwrap().status(),
                StatusCode::BAD_REQUEST
            );
        } else {
            task.abort();
            let _ = task.await;
        }
        drop(sender);
        clean(&m, &dir).await;
    }
}
async fn player_session(store: &Store) -> Id {
    store.run(|db| {
        db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) SELECT 2,'player','Player','enabled',1,verifier FROM accounts WHERE id=1", [])?;
        Ok(())
    }).await.unwrap();
    let mut player = record(1_000_000);
    player
        .data
        .insert("principal".into(), serde_json::json!({"id":2,"epoch":1}));
    SqliteSessions::new(store.clone())
        .create(&mut player)
        .await
        .unwrap();
    player.id
}
#[tokio::test]
async fn upload_route_filename_not_stored_and_actions_admitted() {
    let (dir, store, app, id, m) = setup().await;
    let token = "a".repeat(64);
    let body = multipart(
        &[
            ("csrf", &token),
            ("system", "mork-borg"),
            ("edition", "Synthetic"),
            ("language", "en"),
            ("file", "%PDF-1.7\nsynthetic"),
        ],
        true,
    );
    let response = app
        .clone()
        .oneshot(post_body(id, Body::from(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let path = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let conflict = app
        .clone()
        .oneshot(post_body(id, Body::empty()))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let text = body_text(conflict).await;
    assert!(text.contains("an upload is already staged; open or discard it"));
    assert!(text.contains(&format!("href=\"{path}\"")));
    let player = player_session(&store).await;
    for action in ["set", "render", "inspect", "cancel", "discard"] {
        for case in ["csrf", "missing-csrf", "host", "origin", "non-owner"] {
            let body = if case == "csrf" {
                "csrf=wrong&revision=1".into()
            } else {
                format!("csrf={token}&revision=1")
            };
            let mut req = post_body(id, Body::from(body));
            *req.uri_mut() = format!("{path}/{action}").parse().unwrap();
            req.headers_mut().insert(
                "content-type",
                "application/x-www-form-urlencoded".parse().unwrap(),
            );
            match case {
                "host" => {
                    req.headers_mut()
                        .insert("host", "evil.invalid".parse().unwrap());
                }
                "origin" => {
                    req.headers_mut()
                        .insert("origin", "http://evil.invalid".parse().unwrap());
                }
                "non-owner" => {
                    req.headers_mut()
                        .insert("cookie", format!("mimik_session={player}").parse().unwrap());
                }
                "missing-csrf" => {
                    *req.body_mut() = Body::from("revision=1");
                }
                _ => {}
            }
            let response = app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                response.status(),
                match case {
                    "host" => StatusCode::BAD_REQUEST,
                    _ => StatusCode::FORBIDDEN,
                }
            );
        }
    }
    store
        .run(|db| {
            let mut q = db.prepare("SELECT * FROM source_uploads")?;
            let column_count = q.column_count();
            let mut rows = q.query([])?;
            while let Some(row) = rows.next()? {
                for i in 0..column_count {
                    assert!(!format!("{:?}", row.get_ref(i)?).contains("BUYER-MARKER-NAME"));
                }
            }
            Ok(())
        })
        .await
        .unwrap();
    assert_staging_names_omit_filename(&dir);
    let mut req = post_body(id, Body::from(format!("csrf={token}")));
    *req.uri_mut() = format!("{path}/discard").parse().unwrap();
    req.headers_mut().insert(
        "content-type",
        "application/x-www-form-urlencoded".parse().unwrap(),
    );
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::SEE_OTHER
    );
    clean(&m, &dir).await;
}

fn assert_staging_names_omit_filename(dir: &TestDirectory) {
    for entry in std::fs::read_dir(dir.path().join("staging")).unwrap() {
        let entry = entry.unwrap();
        assert!(
            !entry
                .file_name()
                .to_string_lossy()
                .contains("BUYER-MARKER-NAME")
        );
        if entry.file_type().unwrap().is_dir() {
            for file in std::fs::read_dir(entry.path()).unwrap() {
                assert!(
                    !file
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .contains("BUYER-MARKER-NAME")
                );
            }
        }
    }
}

#[tokio::test]
async fn upload_route_oversize_file_and_no_length_epilogue_are_413_and_cleaned() {
    let (dir, _, app, id, m) = setup().await;
    let token = "a".repeat(64);
    for epilogue in [false, true] {
        let mut prefix = multipart(
            &[
                ("csrf", &token),
                ("system", "mork-borg"),
                ("edition", "Synthetic"),
                ("language", "en"),
            ],
            false,
        ) + "--test\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n%PDF-1.7\n";
        if epilogue {
            prefix.push_str("\r\n--test--\r\n");
        }
        let chunks = stream::unfold(128 * 1024 * 1024 + 65537_usize, |remaining| async move {
            if remaining == 0 {
                return None;
            }
            let n = remaining.min(16384);
            Some((
                Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; n])),
                remaining - n,
            ))
        });
        let body = Body::from_stream(
            stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(prefix)) }).chain(chunks),
        );
        let response = app.clone().oneshot(post_body(id, body)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "epilogue={epilogue}"
        );
        clean(&m, &dir).await;
    }
}

#[tokio::test]
async fn upload_route_invalid_csrf_never_polls_file_frame() {
    let (dir, _, app, id, m) = setup().await;
    let prefix = multipart(&[("csrf", "invalid")], false) + "--test\r\n";
    let file_polled = Arc::new(AtomicBool::new(false));
    let seen = file_polled.clone();
    let file = stream::poll_fn(move |_| {
        seen.store(true, Ordering::Release);
        std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(Bytes::from_static(
            b"Content-Disposition: form-data; name=\"file\"\r\n\r\n%PDF-file bytes",
        ))))
    });
    let body = Body::from_stream(
        stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(prefix)) }).chain(file),
    );
    assert_eq!(
        app.oneshot(post_body(id, body)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert!(!file_polled.load(Ordering::Acquire));
    clean(&m, &dir).await;
}

struct Waiting;
impl Worker for Waiting {
    fn availability(&self) -> worker::Availability {
        worker::Availability::Available
    }
    fn inspect(
        &self,
        _: &std::path::Path,
        cancel: &worker::Cancel,
    ) -> Result<worker::Inspection, worker::Reason> {
        while !cancel.cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(worker::Reason::Cancelled)
    }
    fn render(
        &self,
        _: &std::path::Path,
        _: &[worker::Page],
        _: &worker::Cancel,
        _: &mut (dyn FnMut(worker::RenderedPage) -> Result<(), worker::Error> + Send),
    ) -> Result<(), worker::Reason> {
        unreachable!()
    }
}
#[tokio::test]
async fn upload_route_server_responds_while_job_runs() {
    let (dir, _, app, id, m) = setup_worker(Arc::new(Waiting)).await;
    let token = "a".repeat(64);
    let body = multipart(
        &[
            ("csrf", &token),
            ("system", "mork-borg"),
            ("edition", "Synthetic"),
            ("language", "en"),
            ("file", "%PDF-1.7\nsynthetic"),
        ],
        true,
    );
    let response = app
        .clone()
        .oneshot(post_body(id, Body::from(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let path = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let upload_id = path.rsplit('/').next().unwrap().to_owned();
    for _ in 0..100 {
        let manager = m.clone();
        let upload_id = upload_id.clone();
        let running =
            blocking_test(move || manager.read(&upload_id).unwrap().job.state == "running").await;
        if running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let response = app
        .clone()
        .oneshot(request("/auth/session", Some(id)))
        .await
        .unwrap();
    let manager = m.clone();
    let running =
        blocking_test(move || manager.read(&upload_id).unwrap().job.state == "running").await;
    let mut req = post_body(id, Body::from(format!("csrf={token}")));
    *req.uri_mut() = format!("{path}/discard").parse().unwrap();
    req.headers_mut().insert(
        "content-type",
        "application/x-www-form-urlencoded".parse().unwrap(),
    );
    let discarded = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(running);
    assert_eq!(discarded.status(), StatusCode::SEE_OTHER);
    clean(&m, &dir).await;
}
async fn blocking_test<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

#[tokio::test]
async fn upload_read_routes_require_owner_and_known_upload() {
    let (_dir, store, app, id, _manager) = setup().await;
    let player = player_session(&store).await;
    let base = "/admin/materials/uploads";
    let unknown = format!("{base}/11111111111111111111111111111111");
    for path in [
        base.to_owned(),
        unknown.clone(),
        format!("{unknown}/progress"),
        format!("{unknown}/pages/1/image"),
    ] {
        for (session, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(player), StatusCode::FORBIDDEN),
        ] {
            assert_eq!(
                app.clone()
                    .oneshot(request(&path, session))
                    .await
                    .unwrap()
                    .status(),
                expected,
                "{path}"
            );
        }
        // Material reads intentionally use the same refusal for missing evidence.
        let expected = if path == base {
            StatusCode::OK
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(
            app.clone()
                .oneshot(request(&path, Some(id)))
                .await
                .unwrap()
                .status(),
            expected,
            "{path}"
        );
    }
}

#[tokio::test]
async fn upload_read_routes_redirect_stored_images_and_refuse_symlinks() {
    let (dir, store, app, id, _) = setup().await;
    let path = stage_synthetic_upload(&app, id).await;
    let upload_id = path.rsplit('/').next().unwrap().to_owned();
    let index = app
        .clone()
        .oneshot(request("/admin/materials/uploads", Some(id)))
        .await
        .unwrap();
    assert_eq!(index.status(), StatusCode::SEE_OTHER);
    assert_eq!(index.headers()[header::LOCATION], path);
    for url in [path.clone(), format!("{path}/progress")] {
        assert_eq!(
            app.clone()
                .oneshot(request(&url, Some(id)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    let image_url = format!("{path}/pages/1/image");
    assert_eq!(
        app.clone()
            .oneshot(request(&image_url, Some(id)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    // Install a stored-page fixture directly; worker rendering is covered separately.
    let png_path = dir
        .path()
        .join("staging")
        .join(&upload_id)
        .join("page-1.png");
    std::fs::write(&png_path, b"synthetic PNG bytes").unwrap();
    install_stored_page(&store, upload_id).await;
    let response = app
        .clone()
        .oneshot(request(&image_url, Some(id)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    for (name, value) in [
        (header::CONTENT_TYPE, "image/png"),
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        assert_eq!(response.headers()[name], value);
    }
    assert_eq!(body_text(response).await, "synthetic PNG bytes");
    let player = player_session(&store).await;
    for url in [
        "/admin/materials/uploads".to_owned(),
        path.clone(),
        format!("{path}/progress"),
        image_url.clone(),
    ] {
        for (session, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(player), StatusCode::FORBIDDEN),
        ] {
            assert_eq!(
                app.clone()
                    .oneshot(request(&url, session))
                    .await
                    .unwrap()
                    .status(),
                expected
            );
        }
    }
    #[cfg(unix)]
    {
        let target = dir.path().join("private-target");
        std::fs::write(&target, "DO-NOT-SERVE-TARGET").unwrap();
        std::fs::remove_file(&png_path).unwrap();
        std::os::unix::fs::symlink(&target, &png_path).unwrap();
        let response = app.oneshot(request(&image_url, Some(id))).await.unwrap();
        assert!(!response.status().is_success());
        assert!(!body_text(response).await.contains("DO-NOT-SERVE-TARGET"));
    }
}

async fn stage_synthetic_upload(app: &Router, id: Id) -> String {
    let body = multipart(
        &[
            ("csrf", &"a".repeat(64)),
            ("system", "mork-borg"),
            ("edition", "Synthetic"),
            ("language", "en"),
            ("file", "%PDF-1.7\nsynthetic"),
        ],
        true,
    );
    let response = app
        .clone()
        .oneshot(post_body(id, Body::from(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned()
}

async fn install_stored_page(store: &Store, id: String) {
    store
        .run(move |db| {
            let stats = serde_json::json!({
                "n": 1, "chars": 0, "unmapped": 0, "image_area": 0.0,
                "fonts_substituted": [], "fonts_skipped": 0,
                "incomplete": false, "dropped_lines": 0,
            });
            db.execute(
                "INSERT INTO source_upload_pages
                 (upload_id, pdf_page, rendered_revision, png_sha256, png_bytes,
                  width_px, height_px, dpi, width_points, height_points, transform,
                  stats, settings, text_sha256, text_bytes, renderer_crate_version, mupdf_version)
                 VALUES (?1,1,1,?2,19,1,1,150,1,1,'[1,0,0,1,0,0]',?3,?4,?2,1,?5,?6)",
                rusqlite::params![
                    id,
                    "a".repeat(64),
                    stats.to_string(),
                    serde_json::to_string(&worker::RenderSettings::default()).unwrap(),
                    worker::CRATE_VERSION,
                    worker::MUPDF_VERSION,
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}
