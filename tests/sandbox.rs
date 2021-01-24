use code_sandbox::SandboxBuilder;

#[tokio::test]
async fn basic_sandbox_usage() {
    let mut builder = SandboxBuilder::new("dcchut/code-sandbox-rust-stable", vec!["cargo", "run"])
        .expect("failed to create builder");

    builder
        .mount(
            "/playground/src/main.rs",
            String::from(r#"fn main() { println!("hello world!"); }"#),
        )
        .expect("failed to mount entry point");

    let sb = builder.build().expect("failed to build sandbox");
    let response = sb.execute().await.expect("failed to execute sandbox");

    assert!(response.stdout().contains("hello world!"));
}
