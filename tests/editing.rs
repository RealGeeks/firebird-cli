//! End-to-end tests for page references, the read commands added after
//! phase 1, and editing, validating, publishing and discarding a draft.

use std::error::Error;
use std::path::PathBuf;

use assert_cmd::Command;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use firebird_cli::token::{self, Cache, Token};
use mockito::{Matcher, Mock, Server, ServerGuard};
use predicates::prelude::*;
use serde_json::{json, Value};

type TestResult = Result<(), Box<dyn Error>>;

const KEY: &str = "fb_test_abcdEFGHijklMNOPqrstUVWXyz0123456789";
const ABOUT: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const HOME: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const BLOCK: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const DRAFT_BLOCK: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const DRAFT_PAGE: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
const OTHER: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

struct Env {
    server: ServerGuard,
    config_dir: tempfile::TempDir,
    work: tempfile::TempDir,
}

impl Env {
    /// A mock API with a cached token, so only the routes under test are hit.
    fn new() -> Result<Self, Box<dyn Error>> {
        let env = Self {
            server: Server::new(),
            config_dir: tempfile::tempdir()?,
            work: tempfile::tempdir()?,
        };
        let payload = json!({ "exp": token::now() + 900 });
        let jwt = format!("h.{}.s", URL_SAFE_NO_PAD.encode(payload.to_string()));
        Cache::at(env.config_dir.path().join("tokens.json")).put(
            &token::fingerprint(&env.server.url(), KEY),
            &Token::new(jwt),
            token::now(),
        );
        Ok(env)
    }

    fn cmd(&self) -> Result<Command, Box<dyn Error>> {
        let mut cmd = Command::cargo_bin("firebird-cli")?;
        cmd.env_clear()
            .env(
                "SYSTEMROOT",
                std::env::var("SYSTEMROOT").unwrap_or_default(),
            )
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("FIREBIRD_API_KEY", KEY)
            .env("FIREBIRD_BASE_URL", self.server.url())
            .env("FIREBIRD_CONFIG_DIR", self.config_dir.path());
        Ok(cmd)
    }

    /// Run a command that must succeed and parse its stdout.
    fn json(&self, args: &[&str]) -> Result<Value, Box<dyn Error>> {
        let assert = self.cmd()?.args(args).assert().success();
        Ok(serde_json::from_slice(&assert.get_output().stdout)?)
    }

    /// Run a command that must succeed and return its stdout as text.
    fn text(&self, args: &[&str]) -> Result<String, Box<dyn Error>> {
        let assert = self.cmd()?.args(args).assert().success();
        Ok(String::from_utf8_lossy(&assert.get_output().stdout).into_owned())
    }

    /// Run a command that must fail with `code` and parse the error on stderr.
    fn error(&self, args: &[&str], code: i32) -> Result<Value, Box<dyn Error>> {
        let assert = self.cmd()?.args(args).assert().code(code);
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let line = stderr.lines().last().unwrap_or_default();
        Ok(serde_json::from_str::<Value>(line)?["error"].clone())
    }

    /// `GET /pages`, which is where a slug is looked up.
    fn pages(&mut self, times: usize) -> Mock {
        let pages = json!([
            {"pageUuid": ABOUT, "slug": "about", "status": "active", "metaTitle": "About Us"},
            {"pageUuid": HOME, "slug": "", "status": "active", "metaTitle": "Home"},
            {"pageUuid": OTHER, "slug": "areas/downtown", "status": "unpublished"},
        ]);
        self.server
            .mock("GET", "/pages")
            .with_body(pages.to_string())
            .expect(times)
            .create()
    }

    fn get(&mut self, path: &str, body: &Value) -> Mock {
        self.server
            .mock("GET", path)
            .with_body(body.to_string())
            .expect(1)
            .create()
    }

    /// A write that must arrive with exactly this JSON body, or none.
    fn write(
        &mut self,
        method: &str,
        path: &str,
        sent: Option<&Value>,
        status: usize,
        answer: &Value,
    ) -> Mock {
        let body = match sent {
            Some(sent) => Matcher::Json(sent.clone()),
            None => Matcher::Exact(String::new()),
        };
        self.server
            .mock(method, path)
            .match_body(body)
            .with_status(status)
            .with_body(answer.to_string())
            .expect(1)
            .create()
    }

    fn file(&self, name: &str, body: &str) -> Result<PathBuf, Box<dyn Error>> {
        let path = self.work.path().join(name);
        std::fs::write(&path, body)?;
        Ok(path)
    }
}

fn draft_block(data: &Value) -> Value {
    json!({
        "draftPageUuid": DRAFT_PAGE,
        "data": {
            "draftBlockInstanceUuid": DRAFT_BLOCK,
            "draftPageUuid": DRAFT_PAGE,
            "sourceBlockInstanceUuid": BLOCK,
            "codeName": "info",
            "sortOrder": 1,
            "isDeleted": false,
            "data": data,
        },
    })
}

fn publish_errors() -> Value {
    json!([{
        "draftPageUuid": DRAFT_PAGE,
        "pageTitle": "About Us",
        "blockErrors": [{
            "blockInstanceUuid": DRAFT_BLOCK,
            "blockLabel": "Info",
            "fieldErrors": [{"fieldKey": "title", "fieldLabel": "Title", "message": "Title is required"}],
            "blockErrors": ["Add at least one agent"],
        }],
    }])
}

// --- page references -----------------------------------------------------------

#[test]
fn a_page_is_named_by_uuid_slug_or_home() -> TestResult {
    let mut env = Env::new()?;
    let pages = env.pages(7);

    for reference in ["about", "/about", "/about/", ABOUT] {
        let page = env.json(&["pages", "get", reference])?;
        assert_eq!(page["pageUuid"], ABOUT, "{reference}");
        assert_eq!(page["metaTitle"], "About Us");
    }
    for home in ["/", "home"] {
        assert_eq!(
            env.text(&["pages", "get", home, "-q"])?,
            format!("{HOME}\n")
        );
    }
    assert_eq!(
        env.json(&["pages", "get", "areas/downtown"])?["pageUuid"],
        OTHER
    );
    pages.assert();
    Ok(())
}

#[test]
fn an_unknown_page_is_exit_4_and_lists_the_slugs() -> TestResult {
    let mut env = Env::new()?;
    let pages = env.pages(3);
    let nothing = env
        .server
        .mock("GET", Matcher::Regex("^/pages/.+".into()))
        .expect(0)
        .create();

    let error = env.error(&["pages", "content", "contact"], 4)?;
    assert_eq!(error["code"], "not_found");
    assert!(error["message"].as_str().is_some_and(
        |m| m.contains("no page 'contact'") && m.contains("/, /about, /areas/downtown")
    ));
    env.error(&["draft", "status", "nope"], 4)?;
    // `pages get` reads the list, so there even an unknown uuid is known to be missing
    env.error(&["pages", "get", "99999999-9999-4999-8999-999999999999"], 4)?;
    env.error(&["pages", "content", ""], 2)?;
    pages.assert();
    nothing.assert();
    Ok(())
}

#[test]
fn an_ambiguous_slug_is_exit_2() -> TestResult {
    let mut env = Env::new()?;
    env.server
        .mock("GET", "/pages")
        .with_body(
            json!([{"pageUuid": ABOUT, "slug": "about"}, {"pageUuid": OTHER, "slug": "/about/"}])
                .to_string(),
        )
        .create();
    let error = env.error(&["pages", "content", "about"], 2)?;
    assert!(error["message"]
        .as_str()
        .is_some_and(|m| m.contains("matches 2 pages") && m.contains(ABOUT) && m.contains(OTHER)));
    Ok(())
}

#[test]
fn a_slug_costs_one_lookup_and_a_uuid_none() -> TestResult {
    let mut env = Env::new()?;
    let pages = env.pages(1);
    let content = json!({
        "initialEditorMode": {"mode": "draft", "pageUuid": ABOUT, "draftPageUuid": DRAFT_PAGE},
        "blocks": [{"blockInstanceUuid": DRAFT_BLOCK, "codeName": "info", "sortOrder": 0, "data": {}}],
    });
    let read = env
        .server
        .mock("GET", format!("/pages/{ABOUT}/content").as_str())
        .with_body(content.to_string())
        .expect(3)
        .create();

    assert_eq!(env.json(&["pages", "content", "about"])?, content);
    assert_eq!(env.json(&["pages", "content", ABOUT])?, content);
    assert_eq!(
        env.text(&["pages", "content", &ABOUT.to_uppercase(), "-q"])?,
        format!("{DRAFT_BLOCK}\n")
    );
    pages.assert();
    read.assert();
    Ok(())
}

// --- editing blocks --------------------------------------------------------------

#[test]
fn blocks_add_posts_the_type_and_position() -> TestResult {
    let mut env = Env::new()?;
    let pages = env.pages(1);
    let path = format!("/pages/{ABOUT}/blocks");
    let answer = draft_block(&json!({"title": "Default"}));
    let appended = env.write(
        "POST",
        &path,
        Some(&json!({"blockCodeName": "info"})),
        201,
        &answer,
    );
    assert_eq!(env.json(&["blocks", "add", "about", "info"])?, answer);
    appended.assert();
    pages.assert();

    let placed = env.write(
        "POST",
        &path,
        Some(&json!({"blockCodeName": "hero", "position": 0})),
        201,
        &answer,
    );
    assert_eq!(
        env.text(&["blocks", "add", ABOUT, "hero", "--position", "0", "-q"])?,
        format!("{DRAFT_BLOCK}\n")
    );
    placed.assert();
    Ok(())
}

#[test]
fn blocks_add_with_data_adds_then_patches() -> TestResult {
    let mut env = Env::new()?;
    let added = env.write(
        "POST",
        &format!("/pages/{ABOUT}/blocks"),
        Some(&json!({"blockCodeName": "info", "position": 2})),
        201,
        &draft_block(&json!({"title": "Default"})),
    );
    let data =
        json!({"title": "Welcome home", "button": {"text": "Go", "visible": "show"}, "columns": 3});
    let answer = draft_block(&data);
    let patched = env.write(
        "PATCH",
        &format!("/pages/{ABOUT}/blocks/{DRAFT_BLOCK}/data"),
        Some(&json!({ "data": data })),
        200,
        &answer,
    );

    let out = env.json(&[
        "blocks",
        "add",
        ABOUT,
        "info",
        "--position",
        "2",
        "--data",
        r#"{"title":"From data","button":{"visible":"show"}}"#,
        "--set",
        "title=Welcome home",
        "--set",
        "button.text=Go",
        "--set",
        "columns=3",
    ])?;
    assert_eq!(out, answer);
    added.assert();
    patched.assert();
    Ok(())
}

#[test]
fn a_refused_patch_after_an_add_says_the_block_is_still_there() -> TestResult {
    let mut env = Env::new()?;
    env.write(
        "POST",
        &format!("/pages/{ABOUT}/blocks"),
        Some(&json!({"blockCodeName": "info"})),
        201,
        &draft_block(&json!({})),
    );
    env.write(
        "PATCH",
        &format!("/pages/{ABOUT}/blocks/{DRAFT_BLOCK}/data"),
        Some(&json!({"data": {"bogus": 1}})),
        400,
        &json!({"error": "Unknown field(s) for block \"info\": \"bogus\""}),
    );
    env.cmd()?
        .args(["blocks", "add", ABOUT, "info", "--set", "bogus=1"])
        .assert()
        .code(5)
        .stdout("")
        .stderr(predicate::str::contains(format!(
            "warning: block {DRAFT_BLOCK} was added"
        )))
        .stderr(predicate::str::contains(r#""code":"validation_error""#));

    // bad flags are caught before anything is written
    let nothing = env.server.mock("POST", Matcher::Any).expect(0).create();
    env.error(&["blocks", "add", ABOUT, "info", "--data", "[1]"], 2)?;
    env.error(&["blocks", "add", ABOUT, "info", "--set", "novalue"], 2)?;
    nothing.assert();
    Ok(())
}

#[test]
fn blocks_patch_sends_a_merge_patch_from_data_and_set() -> TestResult {
    let mut env = Env::new()?;
    let pages = env.pages(1);
    let path = format!("/pages/{ABOUT}/blocks/{BLOCK}/data");
    let sent = json!({"data": {
        "title": "Set wins",
        "subtitle": null,
        "tags": ["a", "b"],
        "button": {"text": "Go", "style": "primary"},
        "zip": "02134",
    }});
    let answer = draft_block(&sent["data"]);
    let patched = env
        .server
        .mock("PATCH", path.as_str())
        .match_body(Matcher::Json(sent))
        .match_header("content-type", "application/json")
        .with_body(answer.to_string())
        .expect(3)
        .create();
    let file = env.file(
        "patch.json",
        r#"{"title":"From file","button":{"style":"primary"},"tags":["x"]}"#,
    )?;
    let sets = [
        "--set",
        "title=Set wins",
        "--set",
        "subtitle=null",
        "--set",
        r#"tags=["a","b"]"#,
        "--set",
        "button.text=Go",
        "--set",
        r#"zip="02134""#,
    ];

    let out = env
        .cmd()?
        .args(["blocks", "patch", "about", BLOCK, "--data"])
        .arg(format!("@{}", file.display()))
        .args(sets)
        .assert()
        .success();
    assert_eq!(
        serde_json::from_slice::<Value>(&out.get_output().stdout)?,
        answer
    );
    pages.assert();

    env.cmd()?
        .args(["blocks", "patch", ABOUT, BLOCK, "-d", "-", "-q"])
        .args(sets)
        .write_stdin(r#"{"title":"From stdin","button":{"style":"primary"}}"#)
        .assert()
        .success()
        .stdout(format!("{DRAFT_BLOCK}\n"));
    env.cmd()?
        .args([
            "blocks",
            "patch",
            ABOUT,
            BLOCK,
            "--set",
            "button.style=primary",
        ])
        .args(sets)
        .assert()
        .success();
    patched.assert();

    let nothing = env.server.mock("PATCH", Matcher::Any).expect(0).create();
    let error = env.error(&["blocks", "patch", ABOUT, BLOCK], 2)?;
    assert!(error["message"]
        .as_str()
        .is_some_and(|m| m.contains("--data or --set")));
    let hidden = env.file(".env", r#"{"title":"SECRET"}"#)?;
    env.error(
        &[
            "blocks",
            "patch",
            ABOUT,
            BLOCK,
            "--data",
            &format!("@{}", hidden.display()),
        ],
        2,
    )?;
    nothing.assert();
    Ok(())
}

#[test]
fn a_locked_page_is_exit_6_and_a_refused_field_exit_5() -> TestResult {
    let mut env = Env::new()?;
    let path = format!("/pages/{ABOUT}/blocks/{BLOCK}/data");
    let locked = json!({"error": "Page is locked by another user"});
    env.write(
        "PATCH",
        &path,
        Some(&json!({"data": {"title": "x"}})),
        409,
        &locked,
    );
    let error = env.error(&["blocks", "patch", ABOUT, BLOCK, "--set", "title=x"], 6)?;
    assert_eq!(error["code"], "conflict");
    assert_eq!(error["body"], locked);

    let refused = json!({"error": "Could not resolve block to draft block"});
    env.write(
        "PATCH",
        &path,
        Some(&json!({"data": {"title": "y"}})),
        400,
        &refused,
    );
    let error = env.error(&["blocks", "patch", ABOUT, BLOCK, "--set", "title=y"], 5)?;
    assert_eq!(error["message"], "Could not resolve block to draft block");
    Ok(())
}

#[test]
fn blocks_move_duplicate_and_order_call_their_routes() -> TestResult {
    let mut env = Env::new()?;
    let void = json!({ "draftPageUuid": DRAFT_PAGE });
    let block = format!("/pages/{ABOUT}/blocks/{BLOCK}");

    for direction in ["up", "down", "top", "bottom"] {
        let moved = env.write(
            "POST",
            &format!("{block}/move"),
            Some(&json!({ "direction": direction })),
            200,
            &void,
        );
        assert_eq!(
            env.json(&["blocks", "move", ABOUT, BLOCK, direction])?,
            void
        );
        moved.assert();
    }
    env.cmd()?
        .args(["blocks", "move", ABOUT, BLOCK, "sideways"])
        .assert()
        .code(2);

    let copy = draft_block(&json!({"title": "Copy"}));
    let duplicated = env.write("POST", &format!("{block}/duplicate"), None, 201, &copy);
    assert_eq!(
        env.text(&["blocks", "duplicate", ABOUT, BLOCK, "-q"])?,
        format!("{DRAFT_BLOCK}\n")
    );
    duplicated.assert();

    let ordered = env.write(
        "POST",
        &format!("/pages/{ABOUT}/blocks/order"),
        Some(&json!({"orderedBlockInstanceUuids": [DRAFT_BLOCK, BLOCK, OTHER]})),
        200,
        &void,
    );
    assert_eq!(
        env.text(&["blocks", "order", ABOUT, DRAFT_BLOCK, BLOCK, OTHER, "-q"])?,
        format!("{DRAFT_PAGE}\n")
    );
    ordered.assert();
    env.cmd()?.args(["blocks", "order", ABOUT]).assert().code(2);
    Ok(())
}

// --- confirmation ----------------------------------------------------------------

#[test]
fn destructive_commands_run_unasked_when_no_one_is_at_the_keyboard() -> TestResult {
    let mut env = Env::new()?;
    let discarded = env.write(
        "POST",
        &format!("/pages/{ABOUT}/draft/discard"),
        None,
        200,
        &json!({"ok": true}),
    );
    // no -y and no terminal: a script or agent is not asked, as in geekcli
    assert_eq!(env.json(&["draft", "discard", ABOUT])?, json!({"ok": true}));
    discarded.assert();
    Ok(())
}

#[test]
fn yes_lets_destructive_commands_through() -> TestResult {
    let mut env = Env::new()?;
    let void = json!({ "draftPageUuid": DRAFT_PAGE });

    let removed = env.write(
        "DELETE",
        &format!("/pages/{ABOUT}/blocks/{BLOCK}"),
        None,
        200,
        &void,
    );
    assert_eq!(env.json(&["blocks", "remove", ABOUT, BLOCK, "-y"])?, void);
    removed.assert();

    let discarded = env.write(
        "POST",
        &format!("/pages/{ABOUT}/draft/discard"),
        None,
        200,
        &json!({"ok": true}),
    );
    assert_eq!(
        env.json(&["--yes", "draft", "discard", ABOUT])?,
        json!({"ok": true})
    );
    discarded.assert();

    let draft = json!({"draftPageUuid": DRAFT_PAGE, "pageUuid": ABOUT, "status": "active", "sourceVersionNumber": 3});
    let restored = env.write(
        "POST",
        &format!("/pages/{ABOUT}/versions/3/restore"),
        None,
        200,
        &draft,
    );
    assert_eq!(
        env.text(&["versions", "restore", ABOUT, "3", "-y", "-q"])?,
        format!("{DRAFT_PAGE}\n")
    );
    restored.assert();
    Ok(())
}

// --- drafts ------------------------------------------------------------------------

#[test]
fn draft_list_and_status_read_their_routes() -> TestResult {
    let mut env = Env::new()?;
    let summaries = json!([{
        "pageUuid": ABOUT, "draftPageUuid": DRAFT_PAGE, "pageSlug": "about",
        "pageTitle": "About Us", "createdBy": OTHER, "isOwn": true,
    }]);
    let listed = env
        .server
        .mock("GET", "/pages/drafts/summaries")
        .with_body(summaries.to_string())
        .expect(3)
        .create();
    assert_eq!(env.json(&["draft", "list"])?, summaries);
    assert_eq!(env.text(&["draft", "list", "-q"])?, format!("{ABOUT}\n"));
    assert!(env
        .text(&["draft", "list", "-o", "table"])?
        .contains("About Us"));
    listed.assert();

    let draft = json!({"draftPageUuid": DRAFT_PAGE, "pageUuid": ABOUT, "status": "active", "mode": "batch"});
    let status = env.get(&format!("/pages/{ABOUT}/draft"), &draft);
    assert_eq!(env.json(&["draft", "status", ABOUT])?, draft);
    status.assert();

    // no draft is JSON null and still exit 0
    let none = env.get(&format!("/pages/{HOME}/draft"), &Value::Null);
    assert_eq!(env.json(&["draft", "status", HOME])?, Value::Null);
    none.assert();
    Ok(())
}

#[test]
fn draft_validate_exits_5_when_there_are_errors_and_still_prints_them() -> TestResult {
    let mut env = Env::new()?;
    let path = format!("/pages/{ABOUT}/draft/validate");

    let clean = json!({"hasDraft": true, "errors": []});
    let ok = env.write("POST", &path, None, 200, &clean);
    assert_eq!(env.json(&["draft", "validate", ABOUT])?, clean);
    ok.assert();

    let no_draft = json!({"hasDraft": false, "errors": []});
    env.write(
        "POST",
        &format!("/pages/{HOME}/draft/validate"),
        None,
        200,
        &no_draft,
    );
    env.cmd()?
        .args(["draft", "validate", HOME, "-o", "table"])
        .assert()
        .success()
        .stderr(predicate::str::contains("This page has no draft."));

    let invalid = json!({"hasDraft": true, "errors": publish_errors()});
    let refused = env
        .server
        .mock("POST", path.as_str())
        .with_body(invalid.to_string())
        .expect(3)
        .create();
    let assert = env
        .cmd()?
        .args(["draft", "validate", ABOUT])
        .assert()
        .code(5);
    let output = assert.get_output();
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout)?, invalid);
    let error: Value = serde_json::from_slice(&output.stderr)?;
    assert_eq!(error["error"]["code"], "draft_invalid");
    assert_eq!(error["error"]["exit_code"], 5);

    env.cmd()?
        .args(["draft", "validate", ABOUT, "-o", "table"])
        .assert()
        .code(5)
        .stdout(predicate::str::contains(format!(
            "About Us (draft {DRAFT_PAGE}):"
        )))
        .stdout(predicate::str::contains(format!(
            "  Info [{DRAFT_BLOCK}]: Title (title): Title is required"
        )))
        .stdout(predicate::str::contains(format!(
            "  Info [{DRAFT_BLOCK}]: Add at least one agent"
        )))
        .stderr(predicate::str::contains(
            "error: the draft cannot be published yet",
        ));
    env.cmd()?
        .args(["draft", "validate", ABOUT, "-q"])
        .assert()
        .code(5)
        .stdout(format!("{DRAFT_BLOCK}\n"));
    refused.assert();
    Ok(())
}

#[test]
fn draft_publish_sends_the_message_and_prints_what_went_live() -> TestResult {
    let mut env = Env::new()?;
    let pages = env.pages(1);
    let path = format!("/pages/{ABOUT}/draft/publish");
    let answer = json!({"published": [{
        "pageUuid": ABOUT, "draftPageUuid": DRAFT_PAGE, "slug": "about",
        "versionNumber": 4, "liveUrl": "https://www.example.com/about",
    }]});

    let with_message = env.write(
        "POST",
        &path,
        Some(&json!({"publishMessage": "Update the hero headline"})),
        200,
        &answer,
    );
    assert_eq!(
        env.json(&[
            "draft",
            "publish",
            "about",
            "-y",
            "--message",
            " Update the hero headline "
        ])?,
        answer
    );
    with_message.assert();
    pages.assert();

    let without = env
        .server
        .mock("POST", path.as_str())
        .match_body(Matcher::Json(json!({})))
        .with_body(answer.to_string())
        .expect(2)
        .create();
    assert_eq!(
        env.text(&["draft", "publish", ABOUT, "-y", "-q"])?,
        format!("{ABOUT}\n")
    );
    let table = env.text(&["draft", "publish", ABOUT, "-y", "-o", "table"])?;
    assert!(table.contains("versionNumber") && table.contains("https://www.example.com/about"));
    without.assert();

    env.error(&["draft", "publish", ABOUT, "-y", "-m", "  "], 2)?;
    Ok(())
}

#[test]
fn draft_publish_failures_keep_their_shape() -> TestResult {
    let mut env = Env::new()?;
    let path = format!("/pages/{ABOUT}/draft/publish");
    let publish = ["draft", "publish", ABOUT, "-y"];

    let forbidden = json!({"error": "The site owner handles publishing for this site"});
    env.write("POST", &path, Some(&json!({})), 403, &forbidden);
    let error = env.error(&publish, 3)?;
    assert_eq!(error["code"], "forbidden");
    assert_eq!(error["body"], forbidden);

    let locked = json!({
        "error": "Another user is editing one of these pages",
        "lockedPages": [{"pageUuid": ABOUT, "slug": "about", "lockedBy": "Sam"}],
    });
    env.write("POST", &path, Some(&json!({})), 409, &locked);
    let error = env.error(&publish, 6)?;
    assert_eq!(error["code"], "conflict");
    assert_eq!(error["body"]["lockedPages"][0]["lockedBy"], "Sam");

    let invalid = json!({"error": "Some blocks need attention before publishing", "errors": publish_errors()});
    let refused = env
        .server
        .mock("POST", path.as_str())
        .with_status(422)
        .with_body(invalid.to_string())
        .expect(2)
        .create();
    let error = env.error(&publish, 5)?;
    assert_eq!(error["code"], "validation_error");
    assert_eq!(error["body"], invalid);

    // on a terminal the list reads as lines, not as a JSON blob
    env.cmd()?
        .args(publish)
        .args(["-o", "table"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains(
            "error: validation_error: Some blocks need attention before publishing",
        ))
        .stderr(predicate::str::contains(format!(
            "  About Us (draft {DRAFT_PAGE}):"
        )))
        .stderr(predicate::str::contains(format!(
            "    Info [{DRAFT_BLOCK}]: Title (title): Title is required"
        )))
        .stderr(predicate::str::contains("blockErrors").not());
    refused.assert();
    Ok(())
}

// --- versions ----------------------------------------------------------------------

#[test]
fn versions_list_and_get_read_their_routes() -> TestResult {
    let mut env = Env::new()?;
    let versions = json!([
        {"draftPageUuid": DRAFT_PAGE, "pageUuid": ABOUT, "status": "published", "versionNumber": 2, "publishMessage": "Second"},
        {"draftPageUuid": OTHER, "pageUuid": ABOUT, "status": "published", "versionNumber": 1, "publishMessage": "First"},
    ]);
    let listed = env
        .server
        .mock("GET", format!("/pages/{ABOUT}/versions").as_str())
        .with_body(versions.to_string())
        .expect(2)
        .create();
    assert_eq!(env.json(&["versions", "list", ABOUT])?, versions);
    assert_eq!(env.text(&["versions", "list", ABOUT, "-q"])?, "2\n1\n");
    listed.assert();

    let version = json!({
        "version": versions[0],
        "blocks": [{"blockInstanceUuid": BLOCK, "codeName": "hero", "sortOrder": 0, "data": {}}],
    });
    let read = env
        .server
        .mock("GET", format!("/pages/{ABOUT}/versions/2").as_str())
        .with_body(version.to_string())
        .expect(2)
        .create();
    assert_eq!(env.json(&["versions", "get", ABOUT, "2"])?, version);
    assert_eq!(
        env.text(&["versions", "get", ABOUT, "2", "-q"])?,
        format!("{BLOCK}\n")
    );
    read.assert();
    env.cmd()?
        .args(["versions", "get", ABOUT, "two"])
        .assert()
        .code(2);
    Ok(())
}

// --- settings, media and revisions (read only) ------------------------------------------

#[test]
fn nav_get_reads_the_default_bar_a_named_bar_or_a_pages_bar() -> TestResult {
    let mut env = Env::new()?;
    let default = json!({
        "navBarUuid": OTHER,
        "items": [{"navItemUuid": BLOCK, "label": "Listings"}],
        "navBars": [
            {"navBarUuid": DRAFT_PAGE, "name": "Home bar", "isSiteDefault": false, "pageUuids": [HOME]},
            {"navBarUuid": OTHER, "name": "Site default", "isSiteDefault": true, "pageUuids": []},
        ],
    });
    let home_bar = json!({
        "navBarUuid": DRAFT_PAGE,
        "items": [{"navItemUuid": DRAFT_BLOCK, "label": "Home"}],
        "navBars": default["navBars"],
    });
    // newest mock first: the one with the query, then the bare route
    let bare = env
        .server
        .mock("GET", "/navigation")
        .match_query(Matcher::Missing)
        .with_body(default.to_string())
        .expect(4)
        .create();
    let named = env
        .server
        .mock("GET", "/navigation")
        .match_query(Matcher::UrlEncoded("navBarUuid".into(), DRAFT_PAGE.into()))
        .with_body(home_bar.to_string())
        .expect(2)
        .create();
    let pages = env.pages(1);

    assert_eq!(env.json(&["nav", "get"])?, default);
    assert_eq!(env.text(&["nav", "get", "-q"])?, format!("{BLOCK}\n"));
    assert_eq!(
        env.json(&["nav", "get", "--nav-bar", DRAFT_PAGE])?,
        home_bar
    );
    // the home page has its own bar; the about page falls back to the default
    assert_eq!(env.json(&["nav", "get", "--page", "/"])?, home_bar);
    assert_eq!(env.json(&["nav", "get", "--page", ABOUT])?, default);
    bare.assert();
    named.assert();
    pages.assert();
    env.cmd()?
        .args(["nav", "get", "--page", "/", "--nav-bar", OTHER])
        .assert()
        .code(2);
    Ok(())
}

#[test]
fn nav_anchors_asks_for_the_page_by_query() -> TestResult {
    let mut env = Env::new()?;
    let pages = env.pages(1);
    let anchors = json!([{"blockInstanceUuid": BLOCK, "label": "Hero — Welcome"}]);
    let read = env
        .server
        .mock("GET", "/navigation/anchor-targets")
        .match_query(Matcher::UrlEncoded("pageUuid".into(), ABOUT.into()))
        .with_body(anchors.to_string())
        .expect(2)
        .create();
    assert_eq!(env.json(&["nav", "anchors", "about"])?, anchors);
    assert_eq!(
        env.text(&["nav", "anchors", ABOUT, "-q"])?,
        format!("{BLOCK}\n")
    );
    read.assert();
    pages.assert();
    Ok(())
}

#[test]
fn settings_reads_call_their_routes() -> TestResult {
    let mut env = Env::new()?;
    let cases: [(&[&str], String, Value); 7] = [
        (
            &["styling", "get"],
            "/styling".into(),
            json!({"themeColor": "blue", "spacing": "default"}),
        ),
        (
            &["styling", "fonts"],
            "/styling/fonts".into(),
            json!([{"name": "Barlow", "url": "https://fonts.example.com/barlow"}]),
        ),
        (
            &["footer", "get"],
            "/footer".into(),
            json!({"fullName": "Example Realty", "showLogo": true}),
        ),
        (
            &["headers", "list"],
            "/headers".into(),
            json!([{"headerUuid": BLOCK, "name": "Main", "isSiteDefault": true, "pageUuids": []}]),
        ),
        (
            &["headers", "get", BLOCK],
            format!("/headers/{BLOCK}"),
            json!({"headerUuid": BLOCK, "name": "Main", "isSticky": true}),
        ),
        (
            &["social", "get"],
            "/social-links".into(),
            json!({"facebook": "", "instagram": "example"}),
        ),
        (
            &["areas", "list"],
            "/areas".into(),
            json!([{"areaUuid": OTHER, "name": "Downtown", "routeLocation": "downtown"}]),
        ),
    ];
    for (args, path, body) in cases {
        let read = env.get(&path, &body);
        assert_eq!(env.json(args)?, body, "{args:?}");
        read.assert();
    }

    env.get("/headers", &json!([{"headerUuid": BLOCK, "name": "Main"}]));
    assert_eq!(env.text(&["headers", "list", "-q"])?, format!("{BLOCK}\n"));
    env.get(
        "/styling/fonts",
        &json!([{"name": "Barlow"}, {"name": "Lora"}]),
    );
    assert_eq!(env.text(&["styling", "fonts", "-q"])?, "Barlow\nLora\n");
    env.get("/areas", &json!([{"areaUuid": OTHER, "name": "Downtown"}]));
    assert!(env
        .text(&["areas", "list", "-o", "table"])?
        .contains("Downtown"));
    Ok(())
}

#[test]
fn media_list_passes_its_filters_and_get_reads_one() -> TestResult {
    let mut env = Env::new()?;
    let page = json!({"items": [{"mediaUuid": BLOCK, "filename": "hero.jpg", "mimeType": "image/jpeg"}], "total": 41});
    let filtered = env
        .server
        .mock("GET", "/media")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("query".into(), "front porch".into()),
            Matcher::UrlEncoded("tags".into(), "exterior,porch".into()),
            Matcher::UrlEncoded("mediaType".into(), "image".into()),
            Matcher::UrlEncoded("assetRole".into(), "property".into()),
            Matcher::UrlEncoded("orientation".into(), "landscape".into()),
            Matcher::UrlEncoded("limit".into(), "5".into()),
            Matcher::UrlEncoded("offset".into(), "10".into()),
        ]))
        .with_body(page.to_string())
        .expect(3)
        .create();
    let filters = [
        "--query",
        "front porch",
        "--tags",
        "exterior,porch",
        "--media-type",
        "image",
        "--asset-role",
        "property",
        "--orientation",
        "landscape",
        "--limit",
        "5",
        "--offset",
        "10",
    ];
    let list: Vec<&str> = ["media", "list"].into_iter().chain(filters).collect();
    let quiet: Vec<&str> = list.iter().copied().chain(["-q"]).collect();
    assert_eq!(env.json(&list)?, page);
    assert_eq!(env.text(&quiet)?, format!("{BLOCK}\n"));
    env.cmd()?
        .args(&list)
        .args(["-o", "table"])
        .assert()
        .success()
        .stderr(predicate::str::contains("41 in total"))
        .stdout(predicate::str::contains("hero.jpg"));
    filtered.assert();

    let bare = env
        .server
        .mock("GET", "/media")
        .match_query(Matcher::Missing)
        .with_body(r#"{"items":[],"total":0}"#)
        .create();
    assert_eq!(env.json(&["media", "list"])?["total"], 0);
    bare.assert();

    let media = json!({"mediaUuid": BLOCK, "filename": "hero.jpg", "width": 1920});
    let read = env.get(&format!("/media/{BLOCK}"), &media);
    assert_eq!(env.json(&["media", "get", BLOCK])?, media);
    read.assert();
    Ok(())
}

#[test]
fn revisions_list_passes_type_key_and_paging_and_get_reads_one() -> TestResult {
    let mut env = Env::new()?;
    let page = json!({
        "revisions": [{"revisionUuid": BLOCK, "revisionNumber": 7, "source": "api", "resourceType": "navigation"}],
        "nextCursor": 6,
    });
    let listed = env
        .server
        .mock("GET", "/revisions")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("resourceType".into(), "navigation".into()),
            Matcher::UrlEncoded("resourceKey".into(), OTHER.into()),
            Matcher::UrlEncoded("changeSetUuid".into(), DRAFT_PAGE.into()),
            Matcher::UrlEncoded("cursor".into(), "9".into()),
            Matcher::UrlEncoded("limit".into(), "1".into()),
        ]))
        .with_body(page.to_string())
        .expect(3)
        .create();
    let args = [
        "revisions",
        "list",
        "--type",
        "navigation",
        "--key",
        OTHER,
        "--change-set",
        DRAFT_PAGE,
        "--cursor",
        "9",
        "--limit",
        "1",
    ];
    assert_eq!(env.json(&args)?, page);
    let quiet: Vec<&str> = args.iter().chain(&["-q"]).copied().collect();
    assert_eq!(env.text(&quiet)?, format!("{BLOCK}\n"));
    env.cmd()?
        .args(args)
        .args(["-o", "table"])
        .assert()
        .success()
        .stderr(predicate::str::contains("more: --cursor 6"));
    listed.assert();

    let only_type = env
        .server
        .mock("GET", "/revisions")
        .match_query(Matcher::Exact("resourceType=styling".into()))
        .with_body(r#"{"revisions":[],"nextCursor":null}"#)
        .create();
    assert_eq!(
        env.json(&["revisions", "list", "--type", "styling"])?["nextCursor"],
        Value::Null
    );
    only_type.assert();
    env.cmd()?.args(["revisions", "list"]).assert().code(2);

    let revision =
        json!({"revisionUuid": BLOCK, "revisionNumber": 7, "snapshot": {"themeColor": "blue"}});
    let read = env.get(&format!("/revisions/{BLOCK}"), &revision);
    assert_eq!(env.json(&["revisions", "get", BLOCK])?, revision);
    read.assert();
    Ok(())
}
