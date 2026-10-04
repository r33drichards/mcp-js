//! Exercise real MCP dispatch over an in-memory transport for both services.
use rmcp::{ServerHandler, ServiceExt, model::*};
use serde_json::{Value, json};
use server::{
    engine::Engine,
    mcp::{McpService, StatelessMcpService},
    skills::SkillCatalog,
};
use std::sync::Arc;

async fn exercise(service: impl ServerHandler + Send + Sync + 'static) {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server = tokio::spawn(async move {
        service
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap()
    });
    let client = ().serve(client_io).await.unwrap();
    let info = serde_json::to_value(client.peer_info().unwrap()).unwrap();
    assert_eq!(
        info["capabilities"]["extensions"]["io.modelcontextprotocol/skills"],
        json!({})
    );
    let request = |method: &str, params: Value| {
        ClientRequest::CustomRequest(CustomRequest {
            method: method.into(),
            params: Some(params),
            extensions: Default::default(),
        })
    };
    let result = client
        .send_request(request("skills/list", json!({})))
        .await
        .unwrap();
    let result = serde_json::to_value(result).unwrap();
    assert_eq!(result["skills"][0]["uri"], "skill://workflow/SKILL.md");
    assert_eq!(
        result["skills"][0]["resources"].as_array().unwrap().len(),
        2
    );
    let result = client
        .send_request(request(
            "skills/get",
            json!({"uri": "skill://workflow/SKILL.md"}),
        ))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap()["skill"]["frontmatter"]["name"],
        "workflow"
    );
    assert!(
        client
            .send_request(request(
                "skills/get",
                json!({"uri": "skill://missing/SKILL.md"})
            ))
            .await
            .is_err()
    );
    let docs = client.list_resources(None).await.unwrap();
    assert!(docs.resources.iter().any(|r| r.uri == "docs://readme"));
    assert!(
        docs.resources
            .iter()
            .any(|r| r.uri == "skill://workflow/SKILL.md")
    );
    let file = client
        .read_resource(ReadResourceRequestParams::new(
            "skill://workflow/references/rules.md",
        ))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(file).unwrap()["contents"][0]["text"],
        "Instructions"
    );
    assert!(
        client
            .read_resource(ReadResourceRequestParams::new(
                "skill://workflow/../secret.txt"
            ))
            .await
            .is_err()
    );
    let tools = client.list_tools(None).await.unwrap();
    assert!(tools.tools.iter().any(|t| t.name == "run_js"));
    client.cancel().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn skills_work_in_stateful_and_stateless_mcp_handlers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("workflow/references")).unwrap();
    std::fs::write(
        dir.path().join("workflow/SKILL.md"),
        "---\nname: workflow\ndescription: An example workflow\n---\nRead references/rules.md.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("workflow/references/rules.md"),
        "Instructions",
    )
    .unwrap();
    let skills = Arc::new(SkillCatalog::load(dir.path()).unwrap());
    let engine = Arc::new(Engine::new_stateless(8 * 1024 * 1024, 30, 1));
    exercise(McpService::new(engine.clone(), None).with_skills(skills.clone())).await;
    exercise(StatelessMcpService::new(engine, None).with_skills(skills)).await;
}
