use std::sync::Arc;

use drawcraft_doc::{
    Document, GeometricShape, InkStroke, PageImage, PenSettings, ShapeKind, ShapeLineStyle,
    StrokeId, StrokeSample, TextAlignment, TextItem, sanitize_notebook_title,
};
use drawcraft_sync_protocol::{
    LocalNotebookChange, SyncPushRequest, canonical_document_body_hash, canonical_document_hash,
    canonical_document_meta_hash,
};

fn build_non_roundtrip_document() -> Document {
    let mut shape_color = GeometricShape::default().color;
    shape_color.r = 0.0;
    shape_color.g = 0.5882353;
    shape_color.b = 1.0;
    shape_color.a = 1.0;

    let shape1 = Arc::new(GeometricShape {
        kind: ShapeKind::Rectangle,
        x1: 215.87301220873434,
        y1: 366.6666641437187,
        x2: 936.5079382980282,
        y2: 1246.031750164575,
        angle_degrees: 0.0,
        curve: 0.0,
        color: shape_color,
        width: 2.2,
        line_style: ShapeLineStyle::Dotted,
        ..GeometricShape::default()
    });
    let shape2 = Arc::new(GeometricShape {
        kind: ShapeKind::Line,
        x1: 502.60000026226044,
        y1: 534.7999994754791,
        x2: 239.06853648862398,
        y2: 1018.2444030128719,
        angle_degrees: 15.123456789012346,
        curve: 0.3333333333333333,
        color: shape_color,
        width: 2.4,
        line_style: ShapeLineStyle::Solid,
        ..GeometricShape::default()
    });
    let text1 = Arc::new(TextItem {
        text: "Sample coordinates".to_string(),
        x: 110.09294781206182,
        y: 163.24257011806083,
        size: 20.0,
        angle_degrees: 0.0,
        alignment: TextAlignment::Left,
        max_width: Some(286.45172),
        link_url: Some("http://127.0.0.1:8787".to_string()),
        ..TextItem::default()
    });
    let text2 = Arc::new(TextItem {
        text: "07".to_string(),
        x: 995.7142857142857,
        y: 814.5150733290708,
        size: 14.5,
        angle_degrees: 0.0,
        ..TextItem::default()
    });
    let image = Arc::new(PageImage {
        x: 227.60000000000002,
        y: 487.08279833956493,
        width: 127.11194571869433,
        height: 435.7580486514986,
        angle_degrees: 0.0,
        pixel_width: 4,
        pixel_height: 4,
        png_bytes: Arc::new(vec![0x89, b'P', b'N', b'G']),
    });
    let stroke = InkStroke::new(
        StrokeId(1),
        PenSettings::fountain_pen(),
        vec![
            StrokeSample::from_xy_pressure(215.87301220873434, 366.6666641437187, 0.55),
            StrokeSample::from_xy_pressure(936.5079382980282, 1246.031750164575, 0.72),
        ],
        vec![
            StrokeSample::from_xy_pressure(215.87301220873434, 366.6666641437187, 0.55),
            StrokeSample::from_xy_pressure(936.5079382980282, 1246.031750164575, 0.72),
        ],
    );

    let mut doc = Document {
        id: "224bd623-cca3-4c1c-b247-ca0d28e0cd6c".to_string(),
        title: "Quick Notes".to_string(),
        next_stroke_id: 2,
        ..Document::default()
    };

    let mut page1 = doc.pages[0].clone();
    page1.shapes = vec![Arc::new(GeometricShape {
        kind: ShapeKind::Line,
        x1: 1021.7999994754791,
        y1: 1026.6000002622604,
        x2: 1035.0,
        y2: 1020.0,
        ..GeometricShape::default()
    })];

    doc.pages[0].strokes = vec![Arc::new(stroke)];
    doc.pages[0].shapes = vec![shape1, shape2];
    doc.pages[0].texts = vec![text1, text2];
    doc.pages[0].images = vec![image];
    doc.pages.push(page1);
    doc.load_active_from_pages();
    doc
}

#[test]
fn canonical_hashes_survive_serde_json_wire_roundtrip_without_ulp_drift() {
    let doc = build_non_roundtrip_document();
    let expected_full = canonical_document_hash(&doc);
    let expected_body = canonical_document_body_hash(&doc);
    let expected_meta = canonical_document_meta_hash(&doc);

    let req = SyncPushRequest {
        client_cursor: 5,
        device_name: "Desktop".to_string(),
        changes: vec![LocalNotebookChange {
            notebook_id: doc.id.clone(),
            base_version: 5,
            deleted: false,
            updated_at_ms: 1_795_105_497_566,
            deleted_at_ms: None,
            content_hash: expected_full.clone(),
            body_hash: expected_body.clone(),
            meta_hash: expected_meta.clone(),
            meta_updated_at_ms: 1_795_105_497_566,
            document: Some(doc),
        }],
        folders: Vec::new(),
        studio_prefs: None,
        force_overwrite: false,
    };

    let wire_json = serde_json::to_string(&req).expect("serialize push request");
    let decoded: SyncPushRequest =
        serde_json::from_str(&wire_json).expect("deserialize push request");
    let mut server_doc = decoded.changes[0]
        .document
        .clone()
        .expect("document present in push payload");
    server_doc.title = sanitize_notebook_title(&server_doc.title);
    server_doc.sync_active_to_pages();
    server_doc.load_active_from_pages();

    assert_eq!(canonical_document_hash(&server_doc), expected_full);
    assert_eq!(canonical_document_body_hash(&server_doc), expected_body);
    assert_eq!(canonical_document_meta_hash(&server_doc), expected_meta);
}

#[test]
fn canonical_hashes_match_when_title_requires_server_sanitization() {
    let mut untrimmed = build_non_roundtrip_document();
    untrimmed.title = "  Quick Notes  ".to_string();

    let mut sanitized = untrimmed.clone();
    sanitized.title = sanitize_notebook_title(&sanitized.title);

    assert_eq!(
        canonical_document_hash(&untrimmed),
        canonical_document_hash(&sanitized)
    );
    assert_eq!(
        canonical_document_meta_hash(&untrimmed),
        canonical_document_meta_hash(&sanitized)
    );
}
