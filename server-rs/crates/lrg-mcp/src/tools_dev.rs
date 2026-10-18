//! Tools for testing and developing the plugin from an agent: run the
//! in-Lightroom test suites, read the plugin's and the backend's logs, and —
//! with the developer switch on — run Lua inside the plugin and reload
//! changed modules without restarting Lightroom.
//!
//! This is what lets an agent such as Claude Code test the Lightroom half of
//! LrGeniusAI end to end: change a Lua file, `reload_plugin_modules`,
//! `run_plugin_tests`, read the failures, repeat.

use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, ErrorData as McpError, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use lrg_api::agent_bridge::CallOptions;

use crate::server::{bridge_error, json_result, tool_error, with_warnings, LrGeniusMcp};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunTestsArgs {
    /// Only this suite (the result lists the suites). Default: every suite.
    #[serde(default)]
    pub suite: Option<String>,
    /// Only tests whose name contains this text.
    #[serde(default)]
    pub filter: Option<String>,
    /// Also run the tests that change the catalog. They work on photos the
    /// test picks, undo every change they make, and remove their test
    /// collection. Needs changes allowed in the plugin. Default false.
    #[serde(default)]
    pub include_catalog_writes: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LogsArgs {
    /// "plugin" (the Lightroom plugin's log, default) or "backend" (this
    /// server's log).
    #[serde(default)]
    pub source: Option<String>,
    /// How many of the most recent lines (default 100, at most 2000).
    #[serde(default)]
    pub lines: Option<usize>,
    /// Only lines containing this text (case does not matter).
    #[serde(default)]
    pub contains: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EvalArgs {
    /// Lua run inside the plugin, in a Lightroom async task with all the
    /// plugin's globals (Util, SearchIndexAPI, LrApplication, ...). `return`
    /// a value to get it back as JSON; print(...) output is returned too.
    /// Wrap catalog changes in catalog:withWriteAccessDo yourself.
    pub code: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReloadArgs {
    /// Plugin modules to reload from disk, by file name without ".lua",
    /// e.g. ["Util", "AgentCommands"]. Task*.lua menu files need no reload:
    /// Lightroom reads them each time the menu item runs.
    pub modules: Vec<String>,
}

/// The last `lines` lines of `text` that contain `needle`.
pub(crate) fn tail(text: &str, lines: usize, needle: Option<&str>) -> Vec<String> {
    let needle = needle
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_lowercase);
    let matching: Vec<&str> = text
        .lines()
        .filter(|l| {
            needle
                .as_ref()
                .is_none_or(|n| l.to_lowercase().contains(n.as_str()))
        })
        .collect();
    let start = matching.len().saturating_sub(lines);
    matching[start..].iter().map(|s| s.to_string()).collect()
}

#[tool_router(router = dev_tools, vis = "pub(crate)")]
impl LrGeniusMcp {
    /// Runs the plugin's own tests inside Lightroom — Lua helpers, backend
    /// connectivity, photo identity, the agent bridge, and (optionally)
    /// catalog writes that clean up after themselves — and reports each
    /// result. Use it after changing plugin code. The tests run in the
    /// catalog open in Lightroom: unless the user has said it is a test
    /// catalog, name it and ask before running them there.
    #[tool(
        name = "run_plugin_tests",
        annotations(
            title = "Run the plugin's tests",
            read_only_hint = false,
            destructive_hint = false
        )
    )]
    async fn run_plugin_tests(
        &self,
        Parameters(args): Parameters<RunTestsArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let writes = args.include_catalog_writes.unwrap_or(false);
        let timeout = Duration::from_secs(15 * 60);
        let options = if writes {
            CallOptions::write(timeout)
        } else {
            CallOptions::read(timeout)
        };
        match self
            .lightroom(
                "run_tests",
                json!({
                    "suite": args.suite,
                    "filter": args.filter,
                    "include_writes": writes,
                }),
                options,
                Some(&context),
            )
            .await
        {
            Ok(reply) => {
                let mut result = reply.result;
                // Lead with what failed: a run with 80 passes and one failure
                // should not make anyone scroll for the failure.
                if let Some(results) = result["results"].as_array() {
                    let failures: Vec<Value> = results
                        .iter()
                        .filter(|r| r["ok"].as_bool() == Some(false))
                        .cloned()
                        .collect();
                    result["failures"] = Value::Array(failures);
                }
                Ok(json_result(with_warnings(result, &reply.warnings)))
            }
            Err(e) => Ok(bridge_error(e)),
        }
    }

    /// The most recent lines of the plugin's log (what the Lightroom side
    /// did) or the backend's log (this server).
    #[tool(
        name = "get_logs",
        annotations(title = "Read logs", read_only_hint = true)
    )]
    async fn get_logs(
        &self,
        Parameters(args): Parameters<LogsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let lines = args.lines.unwrap_or(100).clamp(1, 2000);
        let source = args
            .source
            .as_deref()
            .unwrap_or("plugin")
            .trim()
            .to_lowercase();
        match source.as_str() {
            "backend" => match self.api().get("/v1/server/logs").await {
                Ok(logs) => {
                    let text = logs["backend"].as_str().unwrap_or("");
                    Ok(json_result(json!({
                        "source": "backend",
                        "lines": tail(text, lines, args.contains.as_deref()),
                    })))
                }
                Err(e) => Ok(tool_error(format!("Could not read the backend log: {e}"))),
            },
            "plugin" => match self
                .lightroom(
                    "read_log",
                    json!({"lines": lines, "contains": args.contains}),
                    CallOptions::read(Duration::from_secs(30)),
                    None,
                )
                .await
            {
                Ok(reply) => {
                    let mut result = reply.result;
                    result["source"] = json!("plugin");
                    Ok(json_result(with_warnings(result, &reply.warnings)))
                }
                Err(e) => Ok(bridge_error(e)),
            },
            other => Ok(tool_error(format!(
                "source must be \"plugin\" or \"backend\", not \"{other}\"."
            ))),
        }
    }

    /// Developer command: runs Lua inside the LrGeniusAI plugin in Lightroom
    /// and returns the value and printed output. Only works when the plugin
    /// runs from its .lrdevplugin folder with "Allow developer commands"
    /// ticked. Use it to inspect state and probe functions while testing.
    #[tool(
        name = "eval_lua",
        annotations(
            title = "Run Lua in the plugin",
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = false
        )
    )]
    async fn eval_lua(
        &self,
        Parameters(args): Parameters<EvalArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if args.code.trim().is_empty() {
            return Ok(tool_error("code must not be empty."));
        }
        let mut options = CallOptions::read(Duration::from_secs(120));
        options.dev = true;
        match self
            .lightroom(
                "eval_lua",
                json!({"code": args.code}),
                options,
                Some(&context),
            )
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }

    /// Developer command: reloads plugin modules from disk inside the running
    /// Lightroom, so code changes take effect without "Reload plug-in". The
    /// module's table is updated in place, so other modules see the new
    /// functions. Needs developer commands on, like eval_lua.
    #[tool(
        name = "reload_plugin_modules",
        annotations(
            title = "Reload plugin modules",
            read_only_hint = false,
            destructive_hint = false
        )
    )]
    async fn reload_plugin_modules(
        &self,
        Parameters(args): Parameters<ReloadArgs>,
    ) -> Result<CallToolResult, McpError> {
        let modules: Vec<String> = args
            .modules
            .iter()
            .map(|m| m.trim().trim_end_matches(".lua").to_string())
            .filter(|m| !m.is_empty())
            .collect();
        if modules.is_empty() {
            return Ok(tool_error("Name at least one module, e.g. [\"Util\"]."));
        }
        let mut options = CallOptions::read(Duration::from_secs(60));
        options.dev = true;
        match self
            .lightroom("reload_modules", json!({"modules": modules}), options, None)
            .await
        {
            Ok(reply) => Ok(json_result(with_warnings(reply.result, &reply.warnings))),
            Err(e) => Ok(bridge_error(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_filters_then_takes_the_last_lines() {
        let log = "a ERROR one\nb info\nc error two\nd info\ne ERROR three";
        assert_eq!(tail(log, 2, None), vec!["d info", "e ERROR three"]);
        assert_eq!(
            tail(log, 2, Some("error")),
            vec!["c error two", "e ERROR three"]
        );
        assert_eq!(tail(log, 10, Some("  ")).len(), 5);
    }
}
