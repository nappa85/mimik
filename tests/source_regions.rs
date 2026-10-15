use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mimik::{Store, app};
use tower::ServiceExt;

#[path = "support/source_fixture.rs"]
mod fixture;

#[tokio::test]
async fn region_png_maps_top_left_points_to_original_pixels_and_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let mut pixels = Vec::new();
    for y in 0_u8..30 {
        for x in 0_u8..20 {
            pixels.extend_from_slice(&[x, y, 123]);
        }
    }
    let mut image = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut image, 20, 30);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
    }
    std::fs::write(bundle.join("page-1.png"), &image).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    manifest["pages"][0]["image_sha256"] = fixture::hash(&image).into();
    std::fs::write(
        bundle.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let page_id = store.sources().await.unwrap()[0].pages[0].id;
    let before = store.source_page(page_id).await.unwrap();
    let unit = before.units[0].id;
    let router = app(store.clone());
    for (page, unit_id, expected) in [
        (page_id, unit, StatusCode::OK),
        (page_id + 1, unit, StatusCode::NOT_FOUND),
        (page_id, unit + 1, StatusCode::NOT_FOUND),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/sources/pages/{page}/units/{unit_id}/image"))
                    .header("host", "localhost:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == StatusCode::OK {
            assert_eq!(response.headers()["content-type"], "image/png");
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
            let bytes = to_bytes(response.into_body(), 5_000_000).await.unwrap();
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
                .read_info()
                .unwrap();
            let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut decoded).unwrap();
            assert_eq!((info.width, info.height), (9, 3));
            for y in 0..3 {
                for x in 0..9 {
                    assert_eq!(
                        &decoded[(y * 9 + x) * 3..(y * 9 + x + 1) * 3],
                        &[
                            u8::try_from(x + 1).unwrap(),
                            u8::try_from(y + 1).unwrap(),
                            123
                        ]
                    );
                }
            }
        }
    }
    let after = store.source_page(page_id).await.unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.units[0].revision, before.units[0].revision);
    assert_eq!(after.units[0].state, "unreviewed");
    assert!(after.events.is_empty());
    assert_eq!(store.source_image(page_id).await.unwrap(), image);
}

#[tokio::test]
async fn header_keeps_transparent_logo_without_duplicate_visible_title() {
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let router = app(Store::open(&dir.path().join("test.sqlite3")).unwrap());
    let response = router
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "localhost:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = String::from_utf8(
        to_bytes(response.into_body(), 200_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!html.contains("<span>Mimik</span>"));
    assert!(html.contains("alt=\"Mimik home\""));
    let css = include_str!("../assets/app.css");
    let brand = css
        .split(".brand img {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    assert!(!brand.contains("background:"));
}
