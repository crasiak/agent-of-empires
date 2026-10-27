use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn fixture(response: Vec<u8>) -> reqwest::Response {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let _ = stream.write_all(&response).await;
    });
    // Low-level response fixture only; public fetch never allows this target.
    crate::public_network::build_pinned_client("fixture.invalid", &[address])
        .unwrap()
        .get(format!("http://fixture.invalid:{}/", address.port()))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn bounded_transport_rejects_status_encoding_media_and_streamed_excess() {
    for headers in [
        "302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0",
        "403 Forbidden\r\nContent-Type: text/markdown\r\nContent-Length: 0",
        "200 OK\r\nContent-Type: text/html\r\nContent-Length: 0",
        "200 OK\r\nContent-Type: text/markdown\r\nContent-Encoding: gzip\r\nContent-Length: 0",
        "200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 0",
        "200 OK\r\nContent-Type: text/markdown\r\nContent-Length: 10485761",
    ] {
        let response =
            fixture(format!("HTTP/1.1 {headers}\r\nConnection: close\r\n\r\n").into_bytes()).await;
        assert!(read_response(response).await.is_err(), "{headers}");
    }
    let text = "# Exact\r\nUnicode 🌱\n";
    let response = fixture(format!("HTTP/1.1 200 OK\r\nContent-Type: text/markdown; charset=utf-8\r\nConnection: close\r\n\r\n{text}").into_bytes()).await;
    let (bytes, media) = read_response(response).await.unwrap();
    assert_eq!(bytes, text.as_bytes());
    assert_eq!(media, "text/markdown");
    let mut oversized =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n".to_vec();
    oversized.extend(vec![b'a'; RAW_LIMIT + 1]);
    assert!(read_response(fixture(oversized).await).await.is_err());
}

#[tokio::test]
async fn stalled_body_is_bounded_after_response_headers_arrive() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (release, until_release) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/markdown\r\nContent-Length: 1\r\n\r\n",
            )
            .await
            .unwrap();
        let _ = until_release.await;
    });
    let response = crate::public_network::build_pinned_client("fixture.invalid", &[address])
        .unwrap()
        .get(format!("http://fixture.invalid:{}/", address.port()))
        .send()
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), read_response(response))
            .await
            .is_err()
    );
    release.send(()).unwrap();
    server.await.unwrap();
}

fn pdf_fixture() -> Vec<u8> {
    let stream = "BT /F1 12 Tf 20 100 Td (Local document fixture) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
    ];
    let mut pdf = "%PDF-1.4\n".to_owned();
    let mut offsets = vec![0];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
    }
    let xref = pdf.len();
    pdf.push_str("xref\n0 6\n0000000000 65535 f \n");
    for offset in &offsets[1..] {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    ));
    pdf.into_bytes()
}

#[test]
fn local_pdf_extraction_and_failed_extractor_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.pdf");
    std::fs::write(&source, pdf_fixture()).unwrap();
    if let Ok(program) = which::which("pdftotext") {
        let text = extract_pdf(&program, &source, Duration::from_secs(5)).unwrap();
        assert!(std::str::from_utf8(&text)
            .unwrap()
            .contains("Local document fixture"));
        std::fs::write(&source, b"%PDF-broken").unwrap();
        assert!(extract_pdf(&program, &source, Duration::from_secs(5)).is_err());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let extractor = dir.path().join("fake-extractor");
        for script in [
            "#!/bin/sh\nexit 1\n",
            "#!/bin/sh\nexit 0\n",
            "#!/bin/sh\nwhile :; do printf 'overflow source text'; done\n",
            "#!/bin/sh\nwhile :; do :; done\n",
        ] {
            std::fs::write(&extractor, script).unwrap();
            std::fs::set_permissions(&extractor, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(extract_pdf(&extractor, &source, Duration::from_millis(100)).is_err());
        }
    }
}

#[test]
#[serial_test::serial]
fn shared_build_finalizes_before_return_and_cleans_partial_scratch_on_failure() {
    let _app = crate::session::test_support::isolate_app_dir();
    let params = || InstanceParams {
        title: "Document".into(),
        title_typed: false,
        path: String::new(),
        group: "Tailored".into(),
        tool: "pi".into(),
        worktree_enabled: false,
        worktree_branch: None,
        create_new_branch: false,
        base_branch: None,
        sandbox: false,
        sandbox_image: String::new(),
        yolo_mode: false,
        extra_env: Vec::new(),
        extra_args: restrictive_args(),
        command_override: "ledger run pi --profile aoe-document-personal --".into(),
        extra_repo_paths: Vec::new(),
        repo_base_branches: Vec::new(),
        scratch: true,
        fork_seed: None,
    };
    let root = crate::session::scratch::scratch_root().unwrap();
    assert!(build_prepared(params(), &[], &[], "default", |dir| {
        private_write(dir, "source.md", b"partial")?;
        anyhow::bail!("injected extraction failure")
    })
    .is_err());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    let result = build_prepared(params(), &[], &[], "default", |dir| {
        private_write(dir, "source.txt", b"frozen before launch")
    })
    .unwrap();
    assert_eq!(
        std::fs::read(Path::new(&result.instance.project_path).join("source.txt")).unwrap(),
        b"frozen before launch"
    );
    assert!(result.instance.scratch);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let external = tempfile::NamedTempFile::new().unwrap();
        symlink(
            external.path(),
            Path::new(&result.instance.project_path).join("provenance.json"),
        )
        .unwrap();
        assert!(private_write(
            Path::new(&result.instance.project_path),
            "provenance.json",
            b"bad"
        )
        .is_err());
        assert!(read_private(
            Path::new(&result.instance.project_path),
            "provenance.json",
            1024
        )
        .is_err());
    }
}
