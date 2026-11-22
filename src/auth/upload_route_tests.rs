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
        _: &worker::Pins,
        _: &worker::Cancel,
    ) -> Result<worker::Inspection, worker::Reason> {
        unreachable!()
    }
    fn render(
        &self,
        _: &std::path::Path,
        _: &worker::Pins,
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
        _: &worker::Pins,
        _: &worker::Cancel,
    ) -> Result<worker::Inspection, worker::Reason> {
        Err(worker::Reason::Document(worker::Error::Unsupported))
    }
    fn render(
        &self,
        _: &std::path::Path,
        _: &worker::Pins,
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
                // Only a malformed length is refused up front; a large one is a reservation.
                req.headers_mut()
                    .insert("content-length", "not-a-number".parse().unwrap());
                StatusCode::BAD_REQUEST
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
                .oneshot(request("/", Some(id)))
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
    let uploads = Some("/admin/materials/uploads");
    crate::auth::nav::check::assert_header(&text, true, uploads, true, "staged conflict");
    let staged = app.clone().oneshot(request(&path, Some(id))).await.unwrap();
    assert_eq!(staged.status(), StatusCode::OK);
    let staged = body_text(staged).await;
    crate::auth::nav::check::assert_header(&staged, true, uploads, true, "staged upload");
    let player = player_session(&store).await;
    assert_upload_action_admission(&app, &path, id, player, &token).await;
    assert_upload_filename_not_stored(&store).await;
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

async fn assert_upload_action_admission(app: &Router, path: &str, id: Id, player: Id, token: &str) {
    for action in [
        "set",
        "render",
        "inspect",
        "cancel",
        "discard",
        "propose",
        "exclude",
        "exclude-range",
        "remove-exclusion",
    ] {
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
}

async fn assert_upload_filename_not_stored(store: &Store) {
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
}

#[cfg(feature = "dev-endpoint")]
#[tokio::test]
async fn upload_route_propose_confirm_adjust_remove_displayed_boxes() {
    let (dir, app, session, m, path, id) = setup_rendered_upload_route().await;
    let request = |action: &str, fields: &str| upload_action(session, &path, action, fields);
    let response = app
        .clone()
        .oneshot(request("propose", "revision=2"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains("Repeated text: private-text-marker page #"));
    assert!(text.contains("Affected pages: 1-3 (3)"));
    assert!(text.contains("class=\"region-overlay\""));
    assert_eq!(
        text.matches("data-region-rectangle hidden").count(),
        3,
        "unchecked hand-editor defaults must be hidden"
    );
    assert!(!text.contains("http-equiv=\"refresh\""));
    let fingerprint = text
        .split("name=\"confirm_")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let geometry = (1..=3)
        .map(|n| format!("exclude_{n}=1&box_{n}_0=3&box_{n}_1=3&box_{n}_2=97&box_{n}_3=17"))
        .collect::<Vec<_>>()
        .join("&");
    // Confirming a proposal posts its fingerprint only; the server rebuilds the boxes.
    let fields = format!(
        "revision=2&exclusion_revision=0&confirm_{fingerprint}=1&rationale=route-marker&window_from=1&window_to=100"
    );
    assert_eq!(
        app.clone()
            .oneshot(request("exclude", &fields))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        app.clone()
            .oneshot(request("exclude", &fields))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let get = Request::builder()
        .uri(&path)
        .header("host", "127.0.0.1:3000")
        .header("cookie", format!("mimik_session={session}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(get).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = body_text(response).await;
    assert_eq!(text.matches("class=\"exclusion-mask\"").count(), 3);
    assert!(!text.contains("route-marker"));
    let adjusted = format!(
        "revision=2&exclusion_revision=1&rationale=adjusted&window_from=1&window_to=100&{geometry}"
    )
    .replace("box_1_0=3", "box_1_0=4");
    assert_eq!(
        app.clone()
            .oneshot(request("exclude", &adjusted))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(m.read(&id).unwrap().exclusion.unwrap().boxes[&1][0][0], 4.);
    let get = Request::builder()
        .uri(&path)
        .header("host", "127.0.0.1:3000")
        .header("cookie", format!("mimik_session={session}"))
        .body(Body::empty())
        .unwrap();
    let text = body_text(app.clone().oneshot(get).await.unwrap()).await;
    assert!(text.contains("x=\"4\" y=\"3\" width=\"93\" height=\"14\""));
    assert_eq!(
        app.clone()
            .oneshot(request(
                "remove-exclusion",
                "revision=2&exclusion_revision=2&rationale=removed"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    assert!(m.read(&id).unwrap().exclusion.is_none());
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 3);
    assert_eq!(
        app.oneshot(request("discard", "")).await.unwrap().status(),
        StatusCode::SEE_OTHER
    );
    clean(&m, &dir).await;
}

#[cfg(feature = "dev-endpoint")]
#[tokio::test]
async fn upload_route_selection_takes_unordered_ranges_and_select_all() {
    let (dir, app, session, m, path, id) = setup_rendered_upload_route().await;
    let set = |revision: u32, pages: &str| {
        upload_action(
            session,
            &path,
            "set",
            &format!("revision={revision}&system=mork-borg&edition=Synthetic&language=en&{pages}"),
        )
    };
    // Unordered, overlapping and repeated ranges are merged into an ascending subset.
    let status = |response: axum::response::Response| response.status();
    assert_eq!(
        status(app.clone().oneshot(set(2, "pages=3,%202-3")).await.unwrap()),
        StatusCode::SEE_OTHER
    );
    assert_eq!(m.read(&id).unwrap().selected_pages, [2, 3]);
    assert_eq!(
        status(app.clone().oneshot(set(3, "select_all=1")).await.unwrap()),
        StatusCode::SEE_OTHER
    );
    assert_eq!(m.read(&id).unwrap().selected_pages, [1, 2, 3]);
    for bad in [
        "pages=3-1",
        "pages=0",
        "pages=1-20001",
        "pages=x",
        "pages=9",
    ] {
        assert_ne!(
            status(app.clone().oneshot(set(4, bad)).await.unwrap()),
            StatusCode::SEE_OTHER,
            "{bad}"
        );
    }
    assert_eq!(m.read(&id).unwrap().selected_pages, [1, 2, 3]);
    assert_eq!(
        status(
            app.oneshot(upload_action(session, &path, "discard", ""))
                .await
                .unwrap()
        ),
        StatusCode::SEE_OTHER
    );
    clean(&m, &dir).await;
}

fn upload_action(session: Id, path: &str, action: &str, fields: &str) -> Request<Body> {
    let token = "a".repeat(64);
    let mut request = post_body(session, Body::from(format!("csrf={token}&{fields}")));
    *request.uri_mut() = format!("{path}/{action}").parse().unwrap();
    request.headers_mut().insert(
        "content-type",
        "application/x-www-form-urlencoded".parse().unwrap(),
    );
    request
}

#[cfg(feature = "dev-endpoint")]
async fn setup_rendered_upload_route() -> (TestDirectory, Router, Id, Uploads, String, String) {
    use crate::sources::uploads::development::Synthetic;
    setup_rendered_upload_route_worker(Arc::new(Synthetic {
        delay: Duration::ZERO,
    }))
    .await
}

#[cfg(feature = "dev-endpoint")]
async fn setup_rendered_upload_route_worker(
    worker: Arc<dyn Worker>,
) -> (TestDirectory, Router, Id, Uploads, String, String) {
    use crate::sources::uploads::development::PDF;
    let (dir, _, app, session, m) = setup_worker(worker).await;
    let token = "a".repeat(64);
    let body = multipart(
        &[
            ("csrf", &token),
            ("system", "mork-borg"),
            ("edition", "Synthetic"),
            ("language", "en"),
            ("file", std::str::from_utf8(PDF).unwrap()),
        ],
        true,
    );
    let response = app
        .clone()
        .oneshot(post_body(session, Body::from(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let path = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let id = path.rsplit('/').next().unwrap().to_owned();
    wait_uploaded_job(&m, &id).await;
    let response = app
        .clone()
        .oneshot(upload_action(
            session,
            &path,
            "set",
            "revision=1&system=mork-borg&edition=Synthetic&language=en&pages=1-3",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        app.clone()
            .oneshot(upload_action(session, &path, "render", "revision=2"))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    wait_uploaded_job(&m, &id).await;
    (dir, app, session, m, path, id)
}

#[cfg(feature = "dev-endpoint")]
#[tokio::test]
async fn upload_route_same_text_at_two_places_is_one_confirm_control_with_every_box() {
    use crate::sources::uploads::development::Synthetic;
    // The same text at two places on every page: two groups, one fingerprint.
    struct Twin(Synthetic);
    impl Worker for Twin {
        fn availability(&self) -> worker::Availability {
            self.0.availability()
        }
        fn inspect(
            &self,
            path: &std::path::Path,
            pins: &worker::Pins,
            cancel: &worker::Cancel,
        ) -> Result<worker::Inspection, worker::Reason> {
            self.0.inspect(path, pins, cancel)
        }
        fn render(
            &self,
            path: &std::path::Path,
            pins: &worker::Pins,
            pages: &[worker::Page],
            cancel: &worker::Cancel,
            emit: &mut (dyn FnMut(worker::RenderedPage) -> Result<(), worker::Error> + Send),
        ) -> Result<(), worker::Reason> {
            self.0.render(path, pins, pages, cancel, &mut |mut page| {
                let mut second = page.lines[0].clone();
                second.bounds = [5., 100., 95., 110.];
                page.lines.push(second);
                emit(page)
            })
        }
    }
    let (dir, app, session, m, path, id) =
        setup_rendered_upload_route_worker(Arc::new(Twin(Synthetic {
            delay: Duration::ZERO,
        })))
        .await;
    let response = app
        .clone()
        .oneshot(upload_action(session, &path, "propose", "revision=2"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = body_text(response).await;
    // One view of the fingerprint: one confirm, one skip and one absent field, every box shown.
    assert_eq!(text.matches("name=\"confirm_").count(), 1);
    assert_eq!(text.matches("name=\"skip_").count(), 1);
    assert_eq!(text.matches("name=\"absent_").count(), 1);
    assert!(text.contains("Affected pages: 1-3 (3)"));
    assert_eq!(text.matches("class=\"proposal-mask\"").count(), 6);
    let fingerprint = text
        .split("name=\"confirm_")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    // Skipping page 2 on the one control leaves both groups' boxes on pages 1 and 3 only.
    let fields = format!(
        "revision=2&exclusion_revision=0&confirm_{fingerprint}=1&skip_{fingerprint}=2&rationale=twin&window_from=1&window_to=100"
    );
    assert_eq!(
        app.clone()
            .oneshot(upload_action(session, &path, "exclude", &fields))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    let stored = m.read(&id).unwrap().exclusion.unwrap();
    assert_eq!(stored.pages, [1, 3]);
    assert!(stored.boxes.values().all(|b| b.len() == 2));
    // A repeated field is refused whole.
    let twice = format!(
        "revision=2&exclusion_revision=1&confirm_{fingerprint}=1&skip_{fingerprint}=2&skip_{fingerprint}=1&rationale=twin"
    );
    let status = app
        .clone()
        .oneshot(upload_action(session, &path, "exclude", &twice))
        .await
        .unwrap()
        .status();
    assert!(
        status.is_client_error() && status != StatusCode::CONFLICT,
        "{status}"
    );
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 1);
    assert_eq!(
        app.oneshot(upload_action(session, &path, "discard", ""))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    clean(&m, &dir).await;
}

#[cfg(feature = "dev-endpoint")]
#[tokio::test]
async fn upload_route_comparison_budget_explains_manual_remedy_in_html() {
    use crate::sources::uploads::development::Synthetic;
    struct Colliding(Synthetic);
    impl Worker for Colliding {
        fn availability(&self) -> worker::Availability {
            self.0.availability()
        }
        fn inspect(
            &self,
            path: &std::path::Path,
            pins: &worker::Pins,
            cancel: &worker::Cancel,
        ) -> Result<worker::Inspection, worker::Reason> {
            self.0.inspect(path, pins, cancel)
        }
        fn render(
            &self,
            path: &std::path::Path,
            pins: &worker::Pins,
            pages: &[worker::Page],
            cancel: &worker::Cancel,
            emit: &mut (dyn FnMut(worker::RenderedPage) -> Result<(), worker::Error> + Send),
        ) -> Result<(), worker::Reason> {
            self.0.render(path, pins, pages, cancel, &mut |mut page| {
                page.lines = (0..2001)
                    .map(|i| {
                        let left = f64::from(i) / 100.;
                        let top = f64::from(page.n) * 20.;
                        worker::records::TextLine {
                            text: "same text".into(),
                            bounds: [left, top, left + 0.005, top + 1.],
                        }
                    })
                    .collect();
                emit(page)
            })
        }
    }
    let (dir, app, session, m, path, _) =
        setup_rendered_upload_route_worker(Arc::new(Colliding(Synthetic {
            delay: Duration::ZERO,
        })))
        .await;
    let response = app
        .clone()
        .oneshot(upload_action(session, &path, "propose", "revision=2"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let text = body_text(response).await;
    assert!(text.contains("Repeated-text comparison limit reached; draw an exclusion by hand."));
    assert!(text.contains("<nav class=\"menu\"") && text.contains("href=\"/admin/materials\""));
    assert_eq!(
        app.oneshot(upload_action(session, &path, "discard", ""))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    clean(&m, &dir).await;
}

#[cfg(feature = "dev-endpoint")]
async fn wait_uploaded_job(m: &Uploads, id: &str) {
    for _ in 0..500 {
        let m = m.clone();
        let id = id.to_owned();
        let u = tokio::task::spawn_blocking(move || m.read(&id))
            .await
            .unwrap()
            .unwrap();
        if u.job.state == "succeeded" {
            return;
        }
        assert!(!["failed", "cancelled"].contains(&u.job.state.as_str()));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("Synthetic upload job deadline.");
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

const MIB: usize = 1024 * 1024;

/// The fields up to the start of the file's bytes (which begin with `%PDF-1.7`).
fn upload_head() -> String {
    multipart(
        &[
            ("csrf", &"a".repeat(64)),
            ("system", "mork-borg"),
            ("edition", "Synthetic"),
            ("language", "en"),
        ],
        false,
    ) + "--test\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n%PDF-1.7\n"
}
const UPLOAD_TAIL: &str = "\r\n--test--\r\n";
/// `total` bytes in 1 MiB frames cut from one shared block: a generated body that never
/// holds more than one frame; `each` sees the bytes emitted so far.
fn generated(
    total: usize,
    mut each: impl FnMut(usize) + Send + 'static,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> {
    let block = Bytes::from(vec![b'x'; MIB]);
    stream::unfold(0_usize, move |sent| {
        let frame = (sent < total).then(|| block.slice(..(total - sent).min(MIB)));
        if let Some(f) = &frame {
            each(sent + f.len());
        }
        std::future::ready(frame.map(|f| {
            let len = f.len();
            (Ok::<_, std::io::Error>(f), sent + len)
        }))
    })
}
fn whole(
    head: String,
    file: impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    tail: &'static str,
) -> Body {
    Body::from_stream(
        stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(head)) })
            .chain(file)
            .chain(stream::once(async move {
                Ok::<_, std::io::Error>(Bytes::from_static(tail.as_bytes()))
            })),
    )
}
async fn settled(m: &Uploads, dir: &TestDirectory) {
    clean(m, dir).await;
    for _ in 0..100 {
        if m.ledger_usage() == (0, 0) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("reservation leaked: {:?}", m.ledger_usage());
}
async fn wait_inspected(m: &Uploads, id: &str) {
    for _ in 0..500 {
        let (manager, id) = (m.clone(), id.to_owned());
        let u = tokio::task::spawn_blocking(move || manager.read(&id))
            .await
            .unwrap()
            .unwrap();
        if !["queued", "running"].contains(&u.job.state.as_str()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("inspection deadline");
}

#[tokio::test]
async fn upload_route_body_over_128_mib_is_staged_declared_and_chunked() {
    for declared in [true, false] {
        let (_dir, _, app, id, m) = setup().await;
        m.inject_free(Arc::new(std::sync::atomic::AtomicU64::new(1 << 40)));
        let file = 129 * MIB;
        let head = upload_head();
        let length = head.len() + file + UPLOAD_TAIL.len();
        let mut request = post_body(id, whole(head, generated(file, |_| {}), UPLOAD_TAIL));
        if declared {
            request
                .headers_mut()
                .insert("content-length", length.to_string().parse().unwrap());
        }
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "declared={declared}"
        );
        let staged = response.headers()[header::LOCATION].to_str().unwrap();
        let staged = staged.rsplit('/').next().unwrap().to_owned();
        wait_inspected(&m, &staged).await;
        let (manager, read) = (m.clone(), staged.clone());
        let upload = tokio::task::spawn_blocking(move || manager.read(&read))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(upload.pdf_bytes, (file + 9) as u64, "declared={declared}");
        // Every written byte left the reservation and is charged as used.
        assert_eq!(
            m.ledger_usage(),
            (upload.pdf_bytes, 0),
            "declared={declared}"
        );
    }
}

#[tokio::test]
async fn upload_route_free_space_exhausted_mid_stream_is_507_and_everything_is_released() {
    let (dir, _, app, id, m) = setup().await;
    let free = Arc::new(std::sync::atomic::AtomicU64::new(
        crate::sources::staging::MARGIN + 80 * MIB as u64,
    ));
    m.inject_free(free.clone());
    // The disk fills after 30 MiB: the top-up at 65 MiB must be refused.
    let body = whole(
        upload_head(),
        generated(100 * MIB, move |sent| {
            if sent >= 30 * MIB {
                free.store(
                    crate::sources::staging::MARGIN + 5 * MIB as u64,
                    Ordering::SeqCst,
                );
            }
        }),
        UPLOAD_TAIL,
    );
    let response = app.oneshot(post_body(id, body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::INSUFFICIENT_STORAGE);
    // Spool unlinked, reservation and slot released.
    settled(&m, &dir).await;
}

#[tokio::test]
async fn upload_route_declared_length_beyond_free_space_is_507_before_any_file_byte() {
    let (dir, _, app, id, m) = setup().await;
    m.inject_free(Arc::new(std::sync::atomic::AtomicU64::new(
        crate::sources::staging::MARGIN + 100 * MIB as u64,
    )));
    let mut request = post_body(
        id,
        whole(upload_head(), generated(MIB, |_| {}), UPLOAD_TAIL),
    );
    request
        .headers_mut()
        .insert("content-length", (1_u64 << 40).to_string().parse().unwrap());
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INSUFFICIENT_STORAGE);
    // The reservation failed before the file field was read: nothing was spooled or charged.
    assert_eq!(m.ledger_usage(), (0, 0));
    settled(&m, &dir).await;
}

#[test]
fn upload_space_failures_map_to_507() {
    assert_eq!(
        materials::error(&AppError::Invalid("Not enough space.")),
        StatusCode::INSUFFICIENT_STORAGE
    );
    for errno in [libc::ENOSPC, libc::EDQUOT] {
        assert_eq!(
            materials::error(&AppError::SourceIo(std::io::Error::from_raw_os_error(
                errno
            ))),
            StatusCode::INSUFFICIENT_STORAGE
        );
    }
}

/// After admission, a part header that never ends must be refused within a bounded number of
/// bytes: multer buffers a header until its blank line, so without a bound the memory grows
/// with the stream.
async fn assert_endless_header_is_413(head: String) {
    let (dir, _, app, id, m) = setup().await;
    m.inject_free(Arc::new(std::sync::atomic::AtomicU64::new(1 << 40)));
    let emitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = emitted.clone();
    let body = whole(
        head,
        // Short enough that the unbounded (quadratic) buffering still ends the test quickly.
        generated(6 * MIB, move |sent| seen.store(sent, Ordering::SeqCst)),
        "",
    );
    let response = app.oneshot(post_body(id, body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        emitted.load(Ordering::SeqCst) <= 2 * MIB,
        "{} bytes were read",
        emitted.load(Ordering::SeqCst)
    );
    settled(&m, &dir).await;
}
#[tokio::test]
async fn upload_route_endless_header_in_a_form_part_is_413_within_a_bounded_read() {
    let head = multipart(&[("csrf", &"a".repeat(64)), ("system", "mork-borg")], false);
    // The `system` part's header never reaches its blank line.
    let cut = head
        .find("--test\r\nContent-Disposition: form-data; name=\"system\"")
        .unwrap();
    assert_endless_header_is_413(format!("{}--test\r\nContent-Disposition: x", &head[..cut])).await;
}
#[tokio::test]
async fn upload_route_endless_header_after_the_file_is_413_within_a_bounded_read() {
    assert_endless_header_is_413(format!(
        "{}\r\n--test\r\nContent-Disposition: form-data; name=\"other\"; x",
        upload_head()
    ))
    .await;
}

#[tokio::test]
async fn upload_route_invalid_csrf_with_a_huge_declared_length_is_403_not_507() {
    let (dir, _, app, id, m) = setup().await;
    m.inject_free(Arc::new(std::sync::atomic::AtomicU64::new(
        crate::sources::staging::MARGIN + 100 * MIB as u64,
    )));
    let body = Body::from(multipart(&[("csrf", "wrong"), ("file", "%PDF-1.7")], true));
    let mut request = post_body(id, body);
    request
        .headers_mut()
        .insert("content-length", (1_u64 << 40).to_string().parse().unwrap());
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    settled(&m, &dir).await;
}

#[tokio::test]
async fn upload_route_trailer_after_the_closing_boundary_is_bounded() {
    let (dir, _, app, id, m) = setup().await;
    m.inject_free(Arc::new(std::sync::atomic::AtomicU64::new(1 << 40)));
    let body = whole(
        upload_head() + "\r\n--test--\r\n",
        generated(MIB, |_| {}),
        "",
    );
    let response = app.oneshot(post_body(id, body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    settled(&m, &dir).await;
}

// The 15-minute case (a slow client that never idles 30 s) is a paused-time test of the
// counting stream in `uploads.rs`: here blocking writes would let the paused clock jump.
#[tokio::test]
async fn upload_route_thirty_seconds_without_input_is_408_and_released() {
    let (dir, _, app, id, m) = setup().await;
    m.inject_free(Arc::new(std::sync::atomic::AtomicU64::new(1 << 40)));
    tokio::time::pause();
    let stalled = stream::unfold(0_u32, |n| async move {
        if n == 1 {
            return None;
        }
        tokio::time::sleep(Duration::from_secs(31)).await;
        Some((
            Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 1024])),
            n + 1,
        ))
    });
    let response = app
        .oneshot(post_body(id, whole(upload_head(), stalled, UPLOAD_TAIL)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    tokio::time::resume();
    settled(&m, &dir).await;
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
        _: &worker::Pins,
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
        _: &worker::Pins,
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
    let response = app.clone().oneshot(request("/", Some(id))).await.unwrap();
    let manager = m.clone();
    let running =
        blocking_test(move || manager.read(&upload_id).unwrap().job.state == "running").await;
    let proposed = app
        .clone()
        .oneshot(upload_action(id, &path, "propose", "revision=1"))
        .await
        .unwrap();
    assert_eq!(proposed.status(), StatusCode::CONFLICT);
    assert!(
        proposed.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let proposed = body_text(proposed).await;
    assert!(proposed.contains("Wait for the upload job to finish"));
    crate::auth::nav::check::assert_header(
        &proposed,
        true,
        Some("/admin/materials/uploads"),
        true,
        "proposal error",
    );
    assert!(!proposed.contains("http-equiv=\"refresh\""));
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

#[cfg(feature = "dev-endpoint")]
async fn upload_screen(app: &Router, session: Id, location: &str) -> String {
    let get = Request::builder()
        .uri(location)
        .header("host", "127.0.0.1:3000")
        .header("cookie", format!("mimik_session={session}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(get).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_text(response).await
}

#[cfg(feature = "dev-endpoint")]
#[tokio::test]
async fn upload_route_range_boxes_redirect_with_counts_and_refusals_render_the_screen_again() {
    let (dir, app, session, m, path, id) = setup_rendered_upload_route().await;
    let post = |fields: &str| upload_action(session, &path, "exclude-range", fields);
    let status = |app: &Router, fields: String| {
        let request = post(&fields);
        let app = app.clone();
        async move { app.oneshot(request).await.unwrap() }
    };
    let base = "revision=2&p=1&source_page=1&boxes=3+3+97+17%0A5+5+10+10&rationale=range-marker";
    // Add on every selected page: the redirect carries the counts, the screen words them.
    let response = status(&app, format!("{base}&exclusion_revision=0&mode=add&all=1")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        location,
        format!("{path}?p=1&range_changed=3&range_sizes=1")
    );
    assert_eq!(m.read(&id).unwrap().exclusion.unwrap().boxes.len(), 3);
    let text = upload_screen(&app, session, &location).await;
    assert!(text.contains("3 pages changed, 1 page size"), "{text}");
    assert!(text.contains("data-form=\"exclude-range\""));
    assert!(
        !text.contains("range-marker"),
        "a saved rationale is never rendered"
    );
    // A stale exclusion revision is a conflict.
    let stale = status(&app, format!("{base}&exclusion_revision=0&mode=add&all=1")).await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    // A page that is not rendered: 422, the screen again with the posted values and the page.
    let refused = status(
        &app,
        format!("{base}&exclusion_revision=1&mode=add&pages=2-3,7"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let text = body_text(refused).await;
    assert!(
        text.contains("Page 7 is not a selected rendered page"),
        "{text}"
    );
    assert!(
        text.contains("2-3,7")
            && text.contains("3 3 97 17")
            && text.contains(">range-marker</textarea>")
    );
    assert!(text.contains("data-form=\"exclude-range\""));
    // Nothing to change, an unknown mode, a missing acknowledgement and a repeated field.
    let same = status(&app, format!("{base}&exclusion_revision=1&mode=add&all=1")).await;
    assert_eq!(same.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_text(same).await.contains("Nothing to change"));
    assert_range_form_refusals(&app, session, &path, base).await;
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 1);
    // Remove states the pages without a match; clear with its acknowledgement empties it.
    let response = status(
        &app,
        "revision=2&p=1&exclusion_revision=1&mode=remove&pages=1-3&source_page=1&boxes=5+5+10+10&rationale=r2"
            .into(),
    )
    .await;
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("{path}?p=1&range_changed=3&range_sizes=1&range_unmatched=0")
    );
    let response = status(
        &app,
        "revision=2&p=1&exclusion_revision=2&mode=clear&clear_ack=1&all=1&rationale=r3".into(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(m.read(&id).unwrap().exclusion.is_none());
    assert_eq!(
        app.oneshot(upload_action(session, &path, "discard", ""))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    clean(&m, &dir).await;
}

#[cfg(feature = "dev-endpoint")]
#[tokio::test]
async fn upload_route_range_refusal_shows_without_rendered_pages_and_a_bad_address_is_ignored() {
    let (dir, app, session, m, path, id) = setup_rendered_upload_route().await;
    // A malformed count in the address never fails the screen and shows no notice.
    let text = upload_screen(
        &app,
        session,
        &format!("{path}?range_changed=abc&range_sizes=-1"),
    )
    .await;
    assert!(!text.contains("changed,") && text.contains("data-form=\"exclude-range\""));
    let text = upload_screen(
        &app,
        session,
        &format!("{path}?range_changed=2&range_sizes=1&range_unmatched=x"),
    )
    .await;
    assert!(text.contains("2 pages changed, 1 page size."), "{text}");
    assert!(!text.contains("had no matching box"));
    // With no page selected the screen has no manual editors, yet a refused range post still
    // shows its message and the form.
    let revision = m.read(&id).unwrap().revision;
    let response = app
        .clone()
        .oneshot(upload_action(
            session,
            &path,
            "set",
            &format!("revision={revision}&system=mork-borg&edition=Synthetic&language=en&pages="),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let u = m.read(&id).unwrap();
    assert!(u.selected_pages.is_empty(), "no page is selected");
    assert!(
        !upload_screen(&app, session, &path)
            .await
            .contains("data-region-editor")
    );
    let fields = format!(
        "revision={}&exclusion_revision=0&p=1&mode=add&all=1&source_page=1&boxes=3+3+97+17&rationale=why",
        u.revision
    );
    let response = app
        .clone()
        .oneshot(upload_action(session, &path, "exclude-range", &fields))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let text = body_text(response).await;
    assert!(text.contains("data-form=\"exclude-range\""));
    assert!(
        text.contains("Page 1, where the boxes were drawn, is not a selected rendered page."),
        "{text}"
    );
    assert_eq!(
        app.oneshot(upload_action(session, &path, "discard", ""))
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    clean(&m, &dir).await;
}

/// The range form's refusals: each renders the screen again at 422 with its message, and a
/// repeated field is refused whole.
#[cfg(feature = "dev-endpoint")]
async fn assert_range_form_refusals(app: &Router, session: Id, path: &str, base: &str) {
    let post = |fields: String| {
        let request = upload_action(session, path, "exclude-range", &fields);
        let app = app.clone();
        async move { app.oneshot(request).await.unwrap() }
    };
    let head = "revision=2&p=1&exclusion_revision=1&rationale=range-marker";
    let drawn = "source_page=1&boxes=3+3+97+17";
    for (fields, message) in [
        (
            format!("{head}&mode=replace&all=1&{drawn}"),
            "Choose add, remove or clear.",
        ),
        (
            format!("{head}&mode=clear&all=1"),
            "Tick the acknowledgement",
        ),
        (format!("{head}&mode=add&{drawn}"), "Give the pages"),
        (
            format!("{head}&mode=add&all=1&source_page=1&boxes=1+2+3"),
            "Invalid displayed exclusion box.",
        ),
    ] {
        let response = post(fields).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{message}"
        );
        assert!(body_text(response).await.contains(message), "{message}");
    }
    let twice = post(format!(
        "{base}&exclusion_revision=1&mode=add&all=1&mode=add"
    ))
    .await;
    assert_eq!(twice.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
