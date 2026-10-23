//! The MCP server: two tools, `command_list` and `command_run`, over the engine's command registry.
//!
//! Agents get the commands that are marked agent-visible and nothing else: the check lives in `Session::execute`
//! (`Caller::Agent`), so hiding a command from the list and refusing it at run time cannot drift apart. Every
//! command failure is a tool error (`isError`), never a protocol error and never a crash; only arguments that do
//! not fit a tool's own schema (and an unknown tool) are JSON-RPC errors (`-32602`).
//!
//! The session is shared behind a mutex and commands run on blocking threads: they use SQLite and may touch the
//! disk, which must never stall the async threads that carry the protocol.

use std::sync::{Arc, Mutex, PoisonError};

use gymgar_engine::{Caller, CommandKind, CommandSpec, EngineError, Outcome, Session, commands};
use rmcp::handler::server::tool::schema_for_input;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const LIST_TOOL: &str = "command_list";
const RUN_TOOL: &str = "command_run";

/// Shown to the agent once, when the connection starts.
const INSTRUCTIONS: &str = "Gymgar: a workout editor that syncs with Garmin Connect. Everything is a command. Call command_list to see the \
    commands you may run, with their params schemas and whether they can run now, then call command_run with a command id and its params.";

/// Why the MCP server could not run to completion.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AutomationError {
    /// The handshake with the client failed, or the transport could not be opened.
    #[error("cannot start the MCP server: {0}")]
    Start(String),
    /// The connection ended abnormally after it had started.
    #[error("the MCP server stopped abnormally: {0}")]
    Serve(String),
}

/// Arguments of `command_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CommandListArgs {
    /// Only list commands whose id or description contains this text, ignoring case.
    filter: Option<String>,
}

/// Arguments of `command_run`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CommandRunArgs {
    /// The command id from `command_list`, for example `settings.get`.
    id: String,
    /// The command's params, matching its `paramsSchema`. Leave out for a command without params.
    params: Option<Map<String, Value>>,
}

/// One entry of the `command_list` answer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandInfo {
    id: &'static str,
    description: &'static str,
    kind: CommandKind,
    /// Destroys data the user cannot get back.
    destructive: bool,
    /// Whether the command can run right now; when not, `disabled_reason` says why.
    enabled: bool,
    disabled_reason: Option<String>,
    /// JSON Schema of the params object.
    params_schema: Value,
}

/// The body of a failed tool call: the kind names the `EngineError` variant (or `Internal`), so an agent can act on
/// it without parsing the message.
#[derive(Debug, Serialize)]
struct ToolError<'a> {
    error: &'a str,
    message: String,
}

/// The MCP server of one engine session.
#[derive(Clone)]
pub struct GymgarMcp {
    session: Arc<Mutex<Session>>,
}

impl GymgarMcp {
    pub fn new(session: Session) -> GymgarMcp {
        GymgarMcp { session: Arc::new(Mutex::new(session)) }
    }

    /// Runs `f` on a blocking thread with the session locked. A panic in `f` comes back as a `JoinError`; the lock it
    /// poisoned is taken over by the next caller, so one bad command never takes the server down with it.
    async fn with_session<T, F>(&self, f: F) -> Result<T, tokio::task::JoinError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Session) -> T + Send + 'static,
    {
        let session = self.session.clone();
        tokio::task::spawn_blocking(move || {
            let mut session = session.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut session)
        })
        .await
    }

    async fn command_list(&self, args: CommandListArgs) -> CallToolResult {
        let filter = args.filter.map(|text| text.to_lowercase());
        match self.with_session(move |session| list_commands(session, filter.as_deref())).await {
            Ok(list) => json_result(&list),
            Err(error) => failed_result(LIST_TOOL, &error),
        }
    }

    async fn command_run(&self, args: CommandRunArgs) -> CallToolResult {
        let CommandRunArgs { id, params } = args;
        let params = Value::Object(params.unwrap_or_default());
        let command = id.clone();
        self.run_with(&id, move |session| session.execute(&command, params, Caller::Agent)).await
    }

    /// Runs `execute` as command `id` and turns whatever happens, a panic included, into a tool result.
    async fn run_with<F>(&self, id: &str, execute: F) -> CallToolResult
    where
        F: FnOnce(&mut Session) -> Result<Outcome, EngineError> + Send + 'static,
    {
        match self.with_session(execute).await {
            Ok(outcome) => outcome_result(id, outcome),
            Err(error) => failed_result(id, &error),
        }
    }
}

impl ServerHandler for GymgarMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("gymgar", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()?))
    }

    async fn call_tool(&self, request: CallToolRequestParams, _context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        let CallToolRequestParams { name, arguments, .. } = request;
        let result = match name.as_ref() {
            LIST_TOOL => self.command_list(parse_args(LIST_TOOL, arguments)?).await,
            RUN_TOOL => self.command_run(parse_args(RUN_TOOL, arguments)?).await,
            // The MCP spec reports an unknown tool as invalid params.
            other => return Err(ErrorData::invalid_params(format!("unknown tool: {other}"), None)),
        };
        Ok(result.into())
    }
}

/// Serves `session` over this process's stdin and stdout until the client disconnects. Stdout carries the protocol,
/// so nothing else may print to it.
pub async fn serve_stdio(session: Session) -> Result<(), AutomationError> {
    let running = GymgarMcp::new(session).serve(rmcp::transport::io::stdio()).await.map_err(|error| AutomationError::Start(error.to_string()))?;
    running.waiting().await.map_err(|error| AutomationError::Serve(error.to_string()))?;
    Ok(())
}

/// The two tools. Both reject arguments their schema does not list (`additionalProperties: false`).
fn tools() -> Result<Vec<Tool>, ErrorData> {
    let schema_error = |error: String| ErrorData::internal_error(format!("cannot describe the tool arguments: {error}"), None);
    let list = Tool::new(
        LIST_TOOL,
        "List the commands you can run with command_run: id, description, kind (query, mutation or job), whether it is destructive, whether it can run now \
         (and why not), and the JSON Schema of its params. Pass `filter` to narrow the list by text in the id or description.",
        schema_for_input::<CommandListArgs>().map_err(schema_error)?,
    )
    .with_annotations(ToolAnnotations::new().read_only(true).idempotent(true).open_world(false));
    let run = Tool::new(
        RUN_TOOL,
        "Run one command by its id from command_list. `params` must match the command's paramsSchema; unknown params are refused. A failure comes back as a \
         tool error with the error kind and a message. Commands marked destructive cannot be undone.",
        schema_for_input::<CommandRunArgs>().map_err(schema_error)?,
    )
    // Not read-only, and some commands destroy data, so the hint stays at its default of "destructive".
    .with_annotations(ToolAnnotations::new().read_only(false).destructive(true).open_world(false));
    Ok(vec![list, run])
}

/// Parses a tool's arguments; anything that does not fit the tool's own schema is a JSON-RPC invalid-params error.
fn parse_args<T: DeserializeOwned>(tool: &str, arguments: Option<Map<String, Value>>) -> Result<T, ErrorData> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|error| ErrorData::invalid_params(format!("invalid arguments for {tool}: {error}"), None))
}

/// The commands an agent may run, in registry order, narrowed by `filter` (already lowercased).
fn list_commands(session: &Session, filter: Option<&str>) -> Vec<CommandInfo> {
    commands::all()
        .into_iter()
        .filter(|spec| spec.agent && matches_filter(spec, filter))
        .map(|spec| {
            let disabled_reason = (spec.enabled)(session).err();
            CommandInfo {
                id: spec.id,
                description: spec.description,
                kind: spec.kind,
                destructive: spec.destructive,
                enabled: disabled_reason.is_none(),
                disabled_reason,
                params_schema: (spec.params_schema)().into(),
            }
        })
        .collect()
}

fn matches_filter(spec: &CommandSpec, filter: Option<&str>) -> bool {
    filter.is_none_or(|text| spec.id.to_lowercase().contains(text) || spec.description.to_lowercase().contains(text))
}

/// What a command's outcome means for the agent. A command that started a background job cannot be answered: the
/// headless engine has no spawner, so no agent-visible command starts one, and there is no `wait_job` tool.
fn outcome_result(id: &str, outcome: Result<Outcome, EngineError>) -> CallToolResult {
    match outcome {
        Ok(Outcome::Done(value)) => json_result(&value),
        Ok(Outcome::Started(job)) => error_result("Internal", format!("command {id} started background job {}, which this server cannot wait for", job.0)),
        Err(error) => error_result(error_kind(&error), error.to_string()),
    }
}

/// A blocking task that did not finish: it panicked (the usual case) or the runtime is shutting down.
fn failed_result(what: &str, error: &tokio::task::JoinError) -> CallToolResult {
    if error.is_panic() {
        // The panic text is not forwarded: the panic hook has reported it, and it may hold arbitrary data.
        log::error!("{what} panicked");
        error_result("Internal", format!("{what} panicked; the server is still running"))
    } else {
        log::warn!("{what} was cancelled: {error}");
        error_result("Internal", format!("{what} was cancelled because the server is shutting down"))
    }
}

/// The name of the `EngineError` variant. Matched exhaustively, so a new variant fails to compile until it is named.
fn error_kind(error: &EngineError) -> &'static str {
    match error {
        EngineError::UnknownCommand(_) => "UnknownCommand",
        EngineError::BadParams { .. } => "BadParams",
        EngineError::AgentNotAllowed(_) => "AgentNotAllowed",
        EngineError::Disabled { .. } => "Disabled",
        EngineError::Store(_) => "Store",
        EngineError::Secrets(_) => "Secrets",
        EngineError::Garmin(_) => "Garmin",
        EngineError::FileStoreConsentRequired => "FileStoreConsentRequired",
        EngineError::NotConnected => "NotConnected",
        EngineError::KeyMissing => "KeyMissing",
        EngineError::JobPanicked(_) => "JobPanicked",
    }
}

/// A successful result: `value` as JSON text. Text only, because older clients reject `structuredContent` that is
/// not an object, and `command_list` answers with an array.
fn json_result<T: Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string(value) {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(error) => error_result("Internal", format!("cannot encode the result as JSON: {error}")),
    }
}

fn error_result(kind: &str, message: String) -> CallToolResult {
    // Encoding two strings cannot fail; the plain text is the fallback that keeps the error visible regardless.
    let text = serde_json::to_string(&ToolError { error: kind, message: message.clone() }).unwrap_or_else(|_| format!("{kind}: {message}"));
    CallToolResult::error(vec![ContentBlock::text(text)])
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gymgar_engine::testing::{TestConfig, TestHandles, stored_key, test_session, test_session_with};
    use gymgar_engine::{Caller, JobId, Outcome, Session, commands};
    use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, JsonObject, Tool};
    use rmcp::service::{RunningService, ServiceError};
    use rmcp::{RoleClient, ServiceExt};
    use serde_json::{Value, json};

    use super::*;

    type Client = RunningService<RoleClient, ()>;

    /// How long a test waits for the server before it calls the exchange hung.
    const TIMEOUT: Duration = Duration::from_secs(20);

    /// Connects an in-process client to a server over `session`. The returned handle shares the session with the
    /// server, so tests can also call it directly.
    async fn start(session: Session) -> (Client, GymgarMcp) {
        let mcp = GymgarMcp::new(session);
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let server = mcp.clone();
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        let client = tokio::time::timeout(TIMEOUT, ().serve(client_io)).await.expect("the handshake hangs").expect("the client connects");
        (client, mcp)
    }

    fn object(value: Value) -> JsonObject {
        match value {
            Value::Object(map) => map,
            other => panic!("not a JSON object: {other}"),
        }
    }

    async fn call(client: &Client, tool: &'static str, arguments: Value) -> Result<CallToolResult, ServiceError> {
        let request = CallToolRequestParams::new(tool).with_arguments(object(arguments));
        tokio::time::timeout(TIMEOUT, client.call_tool(request)).await.expect("the call hangs")
    }

    /// A tool call that must produce a result (not a protocol error).
    async fn run(client: &Client, id: &str, params: Value) -> CallToolResult {
        call(client, "command_run", json!({ "id": id, "params": params })).await.expect("a tool result")
    }

    fn text(result: &CallToolResult) -> &str {
        assert_eq!(result.content.len(), 1, "one content block expected: {result:?}");
        result.content.first().and_then(|block| block.as_text()).map(|text| text.text.as_str()).expect("a text block")
    }

    fn json_of(result: &CallToolResult) -> Value {
        serde_json::from_str(text(result)).unwrap_or_else(|error| panic!("the text is not JSON ({error}): {}", text(result)))
    }

    fn is_error(result: &CallToolResult) -> bool {
        result.is_error == Some(true)
    }

    fn protocol_error_code(result: Result<CallToolResult, ServiceError>) -> ErrorCode {
        match result {
            Err(ServiceError::McpError(error)) => error.code,
            other => panic!("expected a JSON-RPC error, got {other:?}"),
        }
    }

    /// A session with a working Garmin connection: connected through the engine as the user would.
    fn connected_session() -> (Session, TestHandles) {
        let (mut session, handles) = test_session_with(TestConfig::default());
        let started = session.execute("garmin.connect", json!({ "apiKey": "test-key", "keyTerm": 365 }), Caller::User).expect("connect starts");
        assert!(matches!(started, Outcome::Started(_)), "{started:?}");
        assert_eq!(session.poll_jobs().len(), 1);
        (session, handles)
    }

    async fn tools(client: &Client) -> Vec<Tool> {
        tokio::time::timeout(TIMEOUT, client.list_all_tools()).await.expect("the list hangs").expect("tools/list")
    }

    async fn listed_commands(client: &Client, arguments: Value) -> Vec<Value> {
        let result = call(client, "command_list", arguments).await.expect("a tool result");
        assert!(!is_error(&result), "{result:?}");
        match json_of(&result) {
            Value::Array(items) => items,
            other => panic!("command_list must return an array, got {other}"),
        }
    }

    fn ids(commands: &[Value]) -> Vec<&str> {
        commands.iter().map(|command| command.get("id").and_then(Value::as_str).expect("every command has an id")).collect()
    }

    #[tokio::test]
    async fn lists_two_tools() {
        let (client, _mcp) = start(test_session()).await;
        let tools = tools(&client).await;
        assert_eq!(tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>(), ["command_list", "command_run"]);
        for tool in &tools {
            assert!(tool.description.as_deref().is_some_and(|description| !description.is_empty()), "{} has no description", tool.name);
            assert_eq!(tool.input_schema.get("type"), Some(&json!("object")), "{}", tool.name);
            // Unknown arguments are refused, and the schema tells the agent so.
            assert_eq!(tool.input_schema.get("additionalProperties"), Some(&json!(false)), "{}", tool.name);
        }
        let list = tools.first().and_then(|tool| tool.input_schema.get("properties")).expect("command_list properties");
        assert!(list.get("filter").is_some(), "{list}");
        let run = tools.get(1).expect("command_run");
        let properties = run.input_schema.get("properties").expect("command_run properties");
        assert!(properties.get("id").is_some() && properties.get("params").is_some(), "{properties}");
        assert_eq!(run.input_schema.get("required"), Some(&json!(["id"])));
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn server_info_announces_the_tools_capability() {
        let (client, _mcp) = start(test_session()).await;
        let info = client.peer_info().expect("the server answered initialize");
        assert!(info.capabilities.tools.is_some());
        assert_eq!(info.server_info.as_ref().map(|server| server.name.as_str()), Some("gymgar"));
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn command_list_hides_agent_forbidden() {
        let (client, _mcp) = start(test_session()).await;
        let listed = listed_commands(&client, json!({})).await;
        let listed_ids = ids(&listed);
        for visible in ["garmin.status", "settings.get", "settings.setLanguage"] {
            assert!(listed_ids.contains(&visible), "{visible} is missing from {listed_ids:?}");
        }
        for hidden in ["garmin.connect", "garmin.verify", "garmin.disconnect"] {
            assert!(!listed_ids.contains(&hidden), "{hidden} must not be listed: {listed_ids:?}");
        }
        // Whatever the registry marks agent-visible, and exactly that, in registry order.
        let expected: Vec<&str> = commands::all().into_iter().filter(|spec| spec.agent).map(|spec| spec.id).collect();
        assert_eq!(listed_ids, expected);
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn command_list_describes_each_command() {
        let (client, _mcp) = start(test_session()).await;
        let listed = listed_commands(&client, json!({})).await;
        let set_language = listed.iter().find(|command| command["id"] == "settings.setLanguage").expect("settings.setLanguage is listed");
        assert_eq!(set_language["kind"], "mutation");
        assert_eq!(set_language["destructive"], false);
        assert_eq!(set_language["enabled"], true);
        assert_eq!(set_language["disabledReason"], Value::Null);
        assert!(set_language["description"].as_str().is_some_and(|description| !description.is_empty()));
        assert_eq!(set_language["paramsSchema"]["type"], "object");
        assert_eq!(set_language["paramsSchema"]["properties"]["language"]["enum"], json!(["system", "en", "ru"]));
        let status = listed.iter().find(|command| command["id"] == "garmin.status").expect("garmin.status is listed");
        assert_eq!(status["kind"], "query");
        assert_eq!(status["paramsSchema"]["additionalProperties"], false);
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn command_list_filters_by_id_or_description() {
        let (client, _mcp) = start(test_session()).await;
        let by_id = listed_commands(&client, json!({ "filter": "SETLANG" })).await;
        assert_eq!(ids(&by_id), ["settings.setLanguage"]);
        let by_description = listed_commands(&client, json!({ "filter": "operating system" })).await;
        assert!(ids(&by_description).contains(&"settings.setLanguage"), "{by_description:?}");
        // The area prefix narrows to one area.
        let by_area = listed_commands(&client, json!({ "filter": "garmin." })).await;
        assert!(ids(&by_area).contains(&"garmin.status"), "{by_area:?}");
        assert!(ids(&by_area).iter().all(|id| id.starts_with("garmin.")), "{by_area:?}");
        assert!(listed_commands(&client, json!({ "filter": "no such command" })).await.is_empty());
        // A filter never reveals what the agent may not run.
        assert!(listed_commands(&client, json!({ "filter": "disconnect" })).await.is_empty());
        // An empty filter matches everything.
        assert_eq!(listed_commands(&client, json!({ "filter": "" })).await.len(), listed_commands(&client, json!({})).await.len());
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn command_run_forbidden_is_tool_error() {
        let (session, handles) = connected_session();
        let (client, _mcp) = start(session).await;
        for hidden in ["garmin.disconnect", "garmin.connect", "garmin.verify"] {
            let result = run(&client, hidden, json!({})).await;
            assert!(is_error(&result), "{hidden}: {result:?}");
            assert!(text(&result).contains("AgentNotAllowed"), "{hidden}: {}", text(&result));
        }
        // The connection is untouched: still connected, and the key is still in the secret store.
        let status = run(&client, "garmin.status", json!({})).await;
        assert!(!is_error(&status), "{status:?}");
        assert_eq!(json_of(&status)["status"], "connected");
        let system = handles.system.expect("a system store");
        assert_eq!(stored_key(&system).as_deref(), Some("test-key"));
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn command_run_unknown_command_is_tool_error() {
        let (client, _mcp) = start(test_session()).await;
        let result = run(&client, "settings.nothing", json!({})).await;
        assert!(is_error(&result), "{result:?}");
        assert!(text(&result).contains("UnknownCommand"), "{}", text(&result));
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn unknown_tool_argument_is_invalid_params() {
        let (client, _mcp) = start(test_session()).await;
        let bad_calls = [
            ("command_run", json!({ "id": "settings.get", "extra": 1 })),
            ("command_run", json!({})),
            ("command_run", json!({ "id": 42 })),
            ("command_run", json!({ "id": "settings.get", "params": "ru" })),
            ("command_run", json!({ "id": "settings.get", "params": ["ru"] })),
            ("command_list", json!({ "extra": 1 })),
            ("command_list", json!({ "filter": 7 })),
        ];
        for (tool, arguments) in bad_calls {
            let code = protocol_error_code(call(&client, tool, arguments.clone()).await);
            assert_eq!(code, ErrorCode::INVALID_PARAMS, "{tool} {arguments}");
            assert_eq!(code.0, -32602);
        }
        // The server is fine afterwards.
        assert!(!is_error(&run(&client, "settings.get", json!({})).await));
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn unknown_tool_is_invalid_params() {
        let (client, _mcp) = start(test_session()).await;
        let code = protocol_error_code(call(&client, "command_batch", json!({})).await);
        assert_eq!(code, ErrorCode::INVALID_PARAMS);
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn run_without_params_or_with_null_params() {
        let (client, _mcp) = start(test_session()).await;
        for arguments in [json!({ "id": "settings.get" }), json!({ "id": "settings.get", "params": null }), json!({ "id": "settings.get", "params": {} })] {
            let result = call(&client, "command_run", arguments.clone()).await.expect("a tool result");
            assert!(!is_error(&result), "{arguments}: {result:?}");
            assert_eq!(json_of(&result), json!({ "language": "system" }));
        }
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn set_language_via_mcp() {
        let (client, _mcp) = start(test_session()).await;
        let before = run(&client, "settings.get", json!({})).await;
        assert_eq!(json_of(&before), json!({ "language": "system" }));
        let set = run(&client, "settings.setLanguage", json!({ "language": "ru" })).await;
        assert!(!is_error(&set), "{set:?}");
        assert_eq!(json_of(&set), json!({ "language": "ru" }));
        let after = run(&client, "settings.get", json!({})).await;
        assert!(!is_error(&after), "{after:?}");
        assert_eq!(json_of(&after)["language"], "ru");
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn engine_bad_params_is_tool_error() {
        let (client, _mcp) = start(test_session()).await;
        let bad = [json!({ "language": "de" }), json!({ "language": "ru", "extra": true }), json!({}), json!({ "language": 1 })];
        for params in bad {
            let result = run(&client, "settings.setLanguage", params.clone()).await;
            assert!(is_error(&result), "{params}: {result:?}");
            let body = json_of(&result);
            assert_eq!(body["error"], "BadParams", "{params}");
            assert!(body["message"].as_str().is_some_and(|message| message.contains("settings.setLanguage")), "{body}");
            assert!(text(&result).contains("BadParams"));
        }
        // None of it changed anything.
        let after = run(&client, "settings.get", json!({})).await;
        assert_eq!(json_of(&after)["language"], "system");
        client.cancel().await.expect("the client closes");
    }

    #[tokio::test]
    async fn a_panicking_command_is_a_tool_error_and_the_server_keeps_working() {
        let (client, mcp) = start(test_session()).await;
        // Panics while holding the session lock, which poisons it.
        let result = mcp.run_with("settings.get", |_| panic!("boom")).await;
        assert!(is_error(&result), "{result:?}");
        let body = json_of(&result);
        assert_eq!(body["error"], "Internal");
        assert!(body["message"].as_str().is_some_and(|message| message.contains("settings.get") && message.contains("panicked")), "{body}");
        // The poisoned lock is recovered: the next call works, on the same session.
        let set = run(&client, "settings.setLanguage", json!({ "language": "en" })).await;
        assert!(!is_error(&set), "{set:?}");
        assert_eq!(json_of(&run(&client, "settings.get", json!({})).await)["language"], "en");
        client.cancel().await.expect("the client closes");
    }

    #[test]
    fn a_started_job_is_an_internal_error() {
        let result = outcome_result("garmin.verify", Ok(Outcome::Started(JobId(7))));
        assert!(is_error(&result));
        let body = json_of(&result);
        assert_eq!(body["error"], "Internal");
        assert!(body["message"].as_str().is_some_and(|message| message.contains("garmin.verify")), "{body}");
    }

    #[test]
    fn every_engine_error_names_its_kind() {
        let errors = [
            EngineError::UnknownCommand("x".into()),
            EngineError::BadParams { cmd: "x".into(), msg: "m".into() },
            EngineError::AgentNotAllowed("x".into()),
            EngineError::Disabled { cmd: "x".into(), reason: "r".into() },
            EngineError::Store("s".into()),
            EngineError::Secrets("s".into()),
            EngineError::Garmin(gymgar_engine::GarminAuthError::KeyRejected),
            EngineError::FileStoreConsentRequired,
            EngineError::NotConnected,
            EngineError::KeyMissing,
            EngineError::JobPanicked("x".into()),
        ];
        let kinds: Vec<String> =
            errors.iter().map(|error| json_of(&outcome_result("x", Err(error.clone())))["error"].as_str().expect("a kind").to_owned()).collect();
        assert_eq!(
            kinds,
            [
                "UnknownCommand",
                "BadParams",
                "AgentNotAllowed",
                "Disabled",
                "Store",
                "Secrets",
                "Garmin",
                "FileStoreConsentRequired",
                "NotConnected",
                "KeyMissing",
                "JobPanicked"
            ]
        );
    }
}
