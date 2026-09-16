// Copyright 2026 Hans W. Uhlig
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Model Context Protocol (MCP) server stdio loop, tools, and resource endpoints.
//!
//! Exposes Argus status, reporting, finding queries, and adjudication as standard
//! MCP JSON-RPC 2.0 tools for LLM agent IDE integrations (Claude, Cursor, Antigravity, etc.).

use argus_core::{ArgusError, RunId, Severity};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::Path;

/// JSON-RPC 2.0 Request structure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 Response structure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 Error structure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Run the stdio MCP server event loop until EOF.
pub fn run_mcp_stdio_server(root: &Path) -> Result<(), ArgusError> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = std::io::BufReader::new(stdin.lock());
    let mut writer = std::io::BufWriter::new(stdout.lock());

    let mut line = String::new();
    while reader
        .read_line(&mut line)
        .map_err(|e| ArgusError::invariant("failed to read from stdin").with_source(e))?
        > 0
    {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            if let Some(resp) = handle_json_rpc(root, trimmed) {
                writeln!(writer, "{resp}").map_err(|e| {
                    ArgusError::invariant("failed to write to stdout").with_source(e)
                })?;
                writer
                    .flush()
                    .map_err(|e| ArgusError::invariant("failed to flush stdout").with_source(e))?;
            }
        }
        line.clear();
    }

    Ok(())
}

/// Process an incoming JSON-RPC line and produce an optional JSON-RPC response.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn handle_json_rpc(root: &Path, request_json: &str) -> Option<String> {
    let req: JsonRpcRequest = match serde_json::from_str(request_json) {
        Ok(r) => r,
        Err(e) => {
            let err_resp = JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id: None,
                result: None,
                error: Some(JsonRpcError {
                    code: -32700,
                    message: format!("Parse error: {e}"),
                    data: None,
                }),
            };
            return serde_json::to_string(&err_resp).ok();
        }
    };

    let id = req.id.clone();

    match req.method.as_str() {
        "initialize" => {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                        "resources": {}
                    },
                    "serverInfo": {
                        "name": "argus",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
                error: None,
            };
            serde_json::to_string(&response).ok()
        }

        "notifications/initialized" => {
            // Client acknowledgment, no response needed
            None
        }

        "ping" => {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(serde_json::json!({})),
                error: None,
            };
            serde_json::to_string(&response).ok()
        }

        "tools/list" => {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(serde_json::json!({
                    "tools": [
                        {
                            "name": "argus_status",
                            "description": "Inspect review run state, queue depth, throughput, and error metrics",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "argus_report",
                            "description": "Generate audit report (markdown, html, sarif, json) with optional baseline comparison",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "run_id": {
                                        "type": "string",
                                        "description": "Audit run identifier (default: current run)"
                                    },
                                    "baseline_run_id": {
                                        "type": "string",
                                        "description": "Prior baseline run ID for differential comparison"
                                    },
                                    "format": {
                                        "type": "string",
                                        "enum": ["markdown", "html", "sarif", "json"],
                                        "description": "Report output format (default: markdown)"
                                    },
                                    "severity": {
                                        "type": "string",
                                        "description": "Filter findings by severity (critical, high, medium, low, note)"
                                    },
                                    "dimension": {
                                        "type": "string",
                                        "description": "Filter findings by dimension"
                                    }
                                }
                            }
                        },
                        {
                            "name": "argus_query_findings",
                            "description": "Search and filter candidate findings from an audit run",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "run_id": {
                                        "type": "string",
                                        "description": "Audit run identifier (default: current run)"
                                    },
                                    "policy": {
                                        "type": "string",
                                        "description": "Filter by review policy"
                                    },
                                    "severity": {
                                        "type": "string",
                                        "description": "Filter by severity"
                                    },
                                    "category": {
                                        "type": "string",
                                        "enum": ["new", "resolved", "persistent"],
                                        "description": "Filter by differential category"
                                    },
                                    "query": {
                                        "type": "string",
                                        "description": "Text search query matching titles, descriptions, and locations"
                                    }
                                }
                            }
                        },
                        {
                            "name": "argus_adjudicate",
                            "description": "Record human adjudication (accept, reject, defer) on a candidate finding cluster",
                            "inputSchema": {
                                "type": "object",
                                "required": ["run_id", "finding_id", "decision"],
                                "properties": {
                                    "run_id": {
                                        "type": "string",
                                        "description": "Audit run identifier"
                                    },
                                    "finding_id": {
                                        "type": "string",
                                        "description": "Candidate cluster finding identifier"
                                    },
                                    "decision": {
                                        "type": "string",
                                        "enum": ["accept", "reject", "defer"],
                                        "description": "Adjudication decision"
                                    },
                                    "rationale": {
                                        "type": "string",
                                        "description": "Reviewer rationale or justification"
                                    }
                                }
                            }
                        }
                    ]
                })),
                error: None,
            };
            serde_json::to_string(&response).ok()
        }

        "tools/call" => {
            let params = req.params.unwrap_or(serde_json::Value::Null);
            let tool_name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            let (result_text, is_err) = execute_tool(root, tool_name, &arguments);

            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(serde_json::json!({
                    "content": [
                        {
                            "type": "text",
                            "text": result_text
                        }
                    ],
                    "isError": is_err
                })),
                error: None,
            };
            serde_json::to_string(&response).ok()
        }

        "resources/list" => {
            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: Some(serde_json::json!({
                    "resources": [
                        {
                            "uri": "argus://status",
                            "name": "Argus Review Queue & Pipeline Status",
                            "mimeType": "text/plain"
                        },
                        {
                            "uri": "argus://policies",
                            "name": "Argus Available Review Policies",
                            "mimeType": "text/markdown"
                        }
                    ]
                })),
                error: None,
            };
            serde_json::to_string(&response).ok()
        }

        "resources/read" => {
            let params = req.params.unwrap_or(serde_json::Value::Null);
            let uri = params
                .get("uri")
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            match uri {
                "argus://status" => {
                    let status_text = match crate::status_command(root) {
                        Ok(s) => s,
                        Err(e) => format!("Status unavailable: {e}"),
                    };
                    let response = JsonRpcResponse {
                        jsonrpc: "2.0".to_owned(),
                        id,
                        result: Some(serde_json::json!({
                            "contents": [
                                {
                                    "uri": "argus://status",
                                    "mimeType": "text/plain",
                                    "text": status_text
                                }
                            ]
                        })),
                        error: None,
                    };
                    serde_json::to_string(&response).ok()
                }
                "argus://policies" => {
                    let policies_md = "# Argus Review Policies\n\n- `documentation`: Public API doc comment completeness and accuracy\n- `correctness`: Logic correctness, boundary invariant enforcement, and memory safety\n- `architecture`: Layer boundary enforcement and dependency isolation\n- `conformance`: Normative adherence to governing design documents (ADR, PRD)\n- `maintainability`: Complexity, cohesion, and anti-pattern analysis\n- `optimization`: Algorithmic efficiency, allocations, and resource utilization\n";
                    let response = JsonRpcResponse {
                        jsonrpc: "2.0".to_owned(),
                        id,
                        result: Some(serde_json::json!({
                            "contents": [
                                {
                                    "uri": "argus://policies",
                                    "mimeType": "text/markdown",
                                    "text": policies_md
                                }
                            ]
                        })),
                        error: None,
                    };
                    serde_json::to_string(&response).ok()
                }
                _ => {
                    let err_resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_owned(),
                        id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Unknown resource URI: {uri}"),
                            data: None,
                        }),
                    };
                    serde_json::to_string(&err_resp).ok()
                }
            }
        }

        _ => {
            let err_resp = JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", req.method),
                    data: None,
                }),
            };
            serde_json::to_string(&err_resp).ok()
        }
    }
}

/// Execute an MCP tool call against the Argus repository state.
#[allow(clippy::too_many_lines)]
fn execute_tool(root: &Path, tool_name: &str, arguments: &serde_json::Value) -> (String, bool) {
    match tool_name {
        "argus_status" => match crate::status_command(root) {
            Ok(output) => (output, false),
            Err(e) => (format!("Error querying status: {e}"), true),
        },

        "argus_report" => {
            let mut args = Vec::new();
            if let Some(run_id) = arguments.get("run_id").and_then(|v| v.as_str()) {
                args.push(run_id.to_owned());
            }
            if let Some(base_id) = arguments.get("baseline_run_id").and_then(|v| v.as_str()) {
                args.push("--baseline".to_owned());
                args.push(base_id.to_owned());
            }
            if let Some(fmt) = arguments.get("format").and_then(|v| v.as_str()) {
                args.push("--format".to_owned());
                args.push(fmt.to_owned());
            }
            if let Some(sev) = arguments.get("severity").and_then(|v| v.as_str()) {
                args.push("--severity".to_owned());
                args.push(sev.to_owned());
            }
            if let Some(dim) = arguments.get("dimension").and_then(|v| v.as_str()) {
                args.push("--dimension".to_owned());
                args.push(dim.to_owned());
            }

            match crate::report_command(root, args.into_iter()) {
                Ok(report) => (report, false),
                Err(e) => (format!("Error generating report: {e}"), true),
            }
        }

        "argus_query_findings" => {
            let run_id = arguments
                .get("run_id")
                .and_then(|v| v.as_str())
                .map(|s| s.parse::<RunId>())
                .transpose();

            let run_id = match run_id {
                Ok(Some(r)) => r,
                Ok(None) => match crate::current_run(root) {
                    Ok(r) => r,
                    Err(e) => return (format!("Cannot determine current run: {e}"), true),
                },
                Err(e) => return (format!("Invalid run ID: {e}"), true),
            };

            let queue_res = crate::working_queue(root);
            let findings = match queue_res {
                Ok(queue) => argus_report::extract_all_findings_from_queue(&queue, &run_id)
                    .or_else(|_| {
                        let bundle_dir = root.join(".argus/reviews").join(run_id.as_str());
                        argus_report::extract_all_findings_from_bundle(&bundle_dir, &run_id)
                    }),
                Err(_) => {
                    let bundle_dir = root.join(".argus/reviews").join(run_id.as_str());
                    argus_report::extract_all_findings_from_bundle(&bundle_dir, &run_id)
                }
            };

            let findings = match findings {
                Ok(f) => f,
                Err(e) => return (format!("Error retrieving findings: {e}"), true),
            };

            let policy_filter = arguments.get("policy").and_then(|v| v.as_str());
            let severity_filter =
                arguments
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .and_then(|s| match s.to_lowercase().as_str() {
                        "critical" => Some(Severity::Critical),
                        "high" => Some(Severity::High),
                        "medium" => Some(Severity::Medium),
                        "low" => Some(Severity::Low),
                        "note" => Some(Severity::Note),
                        _ => None,
                    });
            let category_filter = arguments.get("category").and_then(|v| v.as_str());
            let search_query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .map(|q| q.to_lowercase());

            let mut filtered = findings;
            if let Some(pol) = policy_filter {
                let pol_lower = pol.to_lowercase();
                filtered.retain(|f| f.policy.to_lowercase().contains(&pol_lower));
            }
            if let Some(sev) = severity_filter {
                filtered.retain(|f| f.severity == sev);
            }
            if let Some(cat) = category_filter {
                filtered.retain(|f| match cat {
                    "new" => f.category == argus_report::FindingCategory::New,
                    "resolved" => f.category == argus_report::FindingCategory::Resolved,
                    "persistent" => f.category == argus_report::FindingCategory::Persistent,
                    _ => true,
                });
            }
            if let Some(ref q) = search_query {
                filtered.retain(|f| {
                    f.title.to_lowercase().contains(q)
                        || f.description.to_lowercase().contains(q)
                        || f.primary_location
                            .as_deref()
                            .is_some_and(|l| l.to_lowercase().contains(q))
                });
            }

            match serde_json::to_string_pretty(&filtered) {
                Ok(json) => (json, false),
                Err(e) => (format!("Serialization error: {e}"), true),
            }
        }

        "argus_adjudicate" => {
            let run_id = arguments.get("run_id").and_then(|v| v.as_str());
            let finding_id = arguments.get("finding_id").and_then(|v| v.as_str());
            let decision = arguments.get("decision").and_then(|v| v.as_str());
            let rationale = arguments.get("rationale").and_then(|v| v.as_str());

            let (Some(r_id), Some(f_id), Some(dec)) = (run_id, finding_id, decision) else {
                return (
                    "Missing required arguments: run_id, finding_id, and decision are required"
                        .to_owned(),
                    true,
                );
            };

            let mut args = vec![r_id.to_owned(), f_id.to_owned(), format!("--{dec}")];
            if let Some(rat) = rationale {
                args.push("--rationale".to_owned());
                args.push(rat.to_owned());
            }

            match crate::adjudicate_command(root, args.into_iter()) {
                Ok(msg) => (msg, false),
                Err(e) => (format!("Adjudication failed: {e}"), true),
            }
        }

        _ => (format!("Unknown tool: {tool_name}"), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_initialize_and_ping() {
        let temp = tempfile::tempdir().unwrap();

        // 1. initialize
        let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let init_resp = handle_json_rpc(temp.path(), init_req).expect("response");
        let init_val: serde_json::Value = serde_json::from_str(&init_resp).unwrap();
        assert_eq!(init_val["jsonrpc"], "2.0");
        assert_eq!(init_val["id"], 1);
        assert_eq!(init_val["result"]["serverInfo"]["name"], "argus");
        assert_eq!(init_val["result"]["protocolVersion"], "2024-11-05");

        // 2. notifications/initialized (should return None)
        let notify_req = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(handle_json_rpc(temp.path(), notify_req).is_none());

        // 3. ping
        let ping_req = r#"{"jsonrpc":"2.0","id":"ping-42","method":"ping"}"#;
        let ping_resp = handle_json_rpc(temp.path(), ping_req).expect("response");
        let ping_val: serde_json::Value = serde_json::from_str(&ping_resp).unwrap();
        assert_eq!(ping_val["id"], "ping-42");
        assert_eq!(ping_val["result"], serde_json::json!({}));
    }

    #[test]
    fn test_mcp_tools_and_resources_list() {
        let temp = tempfile::tempdir().unwrap();

        // tools/list
        let tools_req = r#"{"jsonrpc":"2.0","id":10,"method":"tools/list"}"#;
        let tools_resp = handle_json_rpc(temp.path(), tools_req).expect("response");
        let tools_val: serde_json::Value = serde_json::from_str(&tools_resp).unwrap();
        let tools = tools_val["result"]["tools"]
            .as_array()
            .expect("tools array");
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"argus_status"));
        assert!(names.contains(&"argus_report"));
        assert!(names.contains(&"argus_query_findings"));
        assert!(names.contains(&"argus_adjudicate"));

        // resources/list
        let res_req = r#"{"jsonrpc":"2.0","id":11,"method":"resources/list"}"#;
        let res_resp = handle_json_rpc(temp.path(), res_req).expect("response");
        let res_val: serde_json::Value = serde_json::from_str(&res_resp).unwrap();
        let resources = res_val["result"]["resources"]
            .as_array()
            .expect("resources array");
        assert_eq!(resources.len(), 2);

        // resources/read argus://policies
        let read_req = r#"{"jsonrpc":"2.0","id":12,"method":"resources/read","params":{"uri":"argus://policies"}}"#;
        let read_resp = handle_json_rpc(temp.path(), read_req).expect("response");
        let read_val: serde_json::Value = serde_json::from_str(&read_resp).unwrap();
        let text = read_val["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("Argus Review Policies"));
        assert!(text.contains("documentation"));
        assert!(text.contains("correctness"));
    }

    #[test]
    fn test_mcp_errors_and_unknown_tool() {
        let temp = tempfile::tempdir().unwrap();

        // Parse error
        let bad_json = r#"{"jsonrpc": "2.0", broken"#;
        let bad_resp = handle_json_rpc(temp.path(), bad_json).expect("response");
        let bad_val: serde_json::Value = serde_json::from_str(&bad_resp).unwrap();
        assert_eq!(bad_val["error"]["code"], -32700);

        // Method not found
        let no_method = r#"{"jsonrpc":"2.0","id":99,"method":"nonexistent"}"#;
        let no_resp = handle_json_rpc(temp.path(), no_method).expect("response");
        let no_val: serde_json::Value = serde_json::from_str(&no_resp).unwrap();
        assert_eq!(no_val["error"]["code"], -32601);

        // Unknown tool call
        let unknown_tool_req = r#"{"jsonrpc":"2.0","id":100,"method":"tools/call","params":{"name":"fake_tool","arguments":{}}}"#;
        let tool_resp = handle_json_rpc(temp.path(), unknown_tool_req).expect("response");
        let tool_val: serde_json::Value = serde_json::from_str(&tool_resp).unwrap();
        assert_eq!(tool_val["result"]["isError"], true);
        assert!(
            tool_val["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Unknown tool")
        );
    }

    #[test]
    fn test_mcp_tools_call_query_findings_and_status() {
        let temp = tempfile::tempdir().unwrap();
        let reviews_dir = temp.path().join(".argus/reviews");
        std::fs::create_dir_all(&reviews_dir).unwrap();

        let run_id = argus_core::RunId::derive([b"mcp-test-run".as_slice()]);
        let run_bundle_dir = reviews_dir.join(run_id.as_str());
        std::fs::create_dir_all(&run_bundle_dir).unwrap();

        let report = argus_report::DocumentationReport {
            schema_version: 1,
            run_id: run_id.clone(),
            policy_version: "documentation-public-api@1".to_owned(),
            summary: argus_report::DocumentationReportSummary::default(),
            finding_clusters: vec![argus_report::DocumentationFindingCluster {
                id: argus_core::FindingId::derive([b"cluster-mcp-1".as_slice()]),
                representative: argus_policies::DocumentationCandidate {
                    title: "Undocumented public function".to_owned(),
                    description: "Public function lacks doc comment".to_owned(),
                    severity: argus_core::Severity::High,
                    confidence: argus_core::Confidence::from_basis_points(9000).unwrap(),
                    dimensions: std::collections::BTreeSet::new(),
                    citations: Vec::new(),
                },
                occurrences: Vec::new(),
                adjudication: argus_core::AdjudicationState::Unreviewed,
            }],
            assessments: Vec::new(),
        };

        std::fs::write(
            run_bundle_dir.join("documentation-report.json"),
            serde_json::to_string(&report).unwrap(),
        )
        .unwrap();

        // Query findings tool call
        let query_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 201,
            "method": "tools/call",
            "params": {
                "name": "argus_query_findings",
                "arguments": {
                    "run_id": run_id.to_string(),
                    "policy": "documentation"
                }
            }
        });

        let resp = handle_json_rpc(temp.path(), &query_req.to_string()).expect("response");
        let val: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(val["result"]["isError"], false);
        let text = val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Undocumented public function"));
        assert!(text.contains("documentation"));
    }
}
