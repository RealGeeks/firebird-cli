//! End-to-end tests for the commands that change the live site: settings
//! saves (read, merge, replace), page settings, areas, restoring a revision,
//! and adding to and describing the media library.

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
const HEADER: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const REVISION: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const PREVIOUS: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
const MEDIA: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

struct Env {
    server: ServerGuard,
    config_dir: tempfile::TempDir,
    work: tempfile::TempDir,
    jwt: String,
}

impl Env {
    /// A mock API with a cached token, so only the routes under test are hit.
    fn new() -> Result<Self, Box<dyn Error>> {
        let payload = json!({ "exp": token::now() + 900 });
        let env = Self {
            server: Server::new(),
            config_dir: tempfile::tempdir()?,
            work: tempfile::tempdir()?,
            jwt: format!("h.{}.s", URL_SAFE_NO_PAD.encode(payload.to_string())),
        };
        Cache::at(env.config_dir.path().join("tokens.json")).put(
            &token::fingerprint(&env.server.url(), KEY),
            &Token::new(env.jwt.clone()),
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

    fn get(&mut self, path: &str, body: &Value, times: usize) -> Mock {
        self.server
            .mock("GET", path)
            .match_query(Matcher::Missing)
            .with_body(body.to_string())
            .expect(times)
            .create()
    }

    /// A write that must arrive with exactly this JSON body.
    fn write(
        &mut self,
        method: &str,
        path: &str,
        sent: &Value,
        status: usize,
        answer: &Value,
    ) -> Mock {
        self.server
            .mock(method, path)
            .match_body(Matcher::Json(sent.clone()))
            .match_header("content-type", "application/json")
            .with_status(status)
            .with_body(answer.to_string())
            .expect(1)
            .create()
    }

    /// Fails the test if any request with this method is made.
    fn never(&mut self, method: &str) -> Mock {
        self.server.mock(method, Matcher::Any).expect(0).create()
    }

    fn file(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, Box<dyn Error>> {
        let path = self.work.path().join(name);
        std::fs::write(&path, bytes)?;
        Ok(path)
    }
}

/// `GET /styling`: the save's fields plus three that are read-only.
fn styling_read() -> Value {
    json!({
        "siteUuid": "11111111-1111-4111-8111-111111111111",
        "themeColor": "blue",
        "primaryColor": null,
        "headingFontName": "Lora",
        "bodyFontName": "Barlow",
        "headingFontUrl": "https://fonts.example.com/lora",
        "bodyFontUrl": "https://fonts.example.com/barlow",
        "cardRadius": "0.75rem",
        "buttonRadius": "0.75rem",
        "spacing": "default",
        "colorMode": "system",
    })
}

/// What `PUT /styling` must receive for `changes`: every save field, no
/// read-only one.
fn styling_body(changes: &Value) -> Value {
    let mut body = json!({
        "themeColor": "blue",
        "primaryColor": null,
        "headingFontName": "Lora",
        "bodyFontName": "Barlow",
        "cardRadius": "0.75rem",
        "buttonRadius": "0.75rem",
        "spacing": "default",
        "colorMode": "system",
    });
    if let (Some(body), Some(changes)) = (body.as_object_mut(), changes.as_object()) {
        body.extend(changes.clone());
    }
    body
}

fn saved(key: &str, object: &Value) -> Value {
    json!({ key: object, "revisionUuid": REVISION })
}

// --- settings: read, merge, replace ------------------------------------------------

#[test]
fn styling_set_reads_then_replaces_with_only_the_change_applied() -> TestResult {
    let mut env = Env::new()?;
    let read = env.get("/styling", &styling_read(), 2);
    let no_history_lookup = env
        .server
        .mock("GET", "/revisions")
        .match_query(Matcher::Any)
        .expect(0)
        .create();
    let body = styling_body(&json!({"spacing": "compact", "primaryColor": "#112233"}));
    let answer = saved("styling", &body);
    let put = env
        .server
        .mock("PUT", "/styling")
        .match_body(Matcher::Json(body))
        .match_header("authorization", format!("Bearer {}", env.jwt).as_str())
        .with_body(answer.to_string())
        .expect(2)
        .create();

    // piped, so no prompt and no -y needed
    let out = env.json(&["styling", "set", "spacing=compact", "primaryColor=#112233"])?;
    assert_eq!(out, answer);
    assert_eq!(
        env.text(&[
            "styling",
            "set",
            "primaryColor=#112233",
            "spacing=compact",
            "-y",
            "-q"
        ])?,
        format!("{REVISION}\n")
    );
    read.assert();
    put.assert();
    no_history_lookup.assert();
    Ok(())
}

#[test]
fn data_and_fields_merge_and_the_read_can_be_piped_back() -> TestResult {
    let mut env = Env::new()?;
    env.get("/styling", &styling_read(), 2);
    let body =
        styling_body(&json!({"spacing": "spacious", "colorMode": "dark", "themeColor": null}));
    let put = env
        .server
        .mock("PUT", "/styling")
        .match_body(Matcher::Json(body.clone()))
        .with_body(saved("styling", &body).to_string())
        .expect(2)
        .create();

    // a field argument wins over the same field in --data
    env.json(&[
        "styling",
        "set",
        "--data",
        r#"{"spacing":"compact","colorMode":"dark"}"#,
        "spacing=spacious",
        "themeColor=null",
    ])?;

    // the whole read, edited, on stdin: its read-only fields are not sent
    let mut edited = styling_read();
    edited["spacing"] = json!("spacious");
    edited["colorMode"] = json!("dark");
    edited["themeColor"] = Value::Null;
    env.cmd()?
        .args(["styling", "set", "--data", "-"])
        .write_stdin(edited.to_string())
        .assert()
        .success();
    put.assert();
    Ok(())
}

#[test]
fn nothing_to_change_and_unknown_fields_are_exit_2_and_write_nothing() -> TestResult {
    let mut env = Env::new()?;
    let read = env.get("/styling", &styling_read(), 2);
    let no_write = env.never("PUT");

    let error = env.error(&["styling", "set"], 2)?;
    assert_eq!(
        error["message"],
        "nothing to change: give key=value fields or --data"
    );
    env.error(&["social", "set"], 2)?;
    env.error(&["footer", "set"], 2)?;
    env.error(&["headers", "update", HEADER], 2)?;

    let error = env.error(&["styling", "set", "spacng=compact"], 2)?;
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains("no field 'spacng'"), "{message}");
    assert!(message.contains("spacing, colorMode"), "{message}");
    let error = env.error(&["styling", "set", "siteUuid=x"], 2)?;
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains("only read-only fields"), "{message}");
    env.error(&["styling", "set", "novalue"], 2)?;

    read.assert();
    no_write.assert();
    Ok(())
}

#[test]
fn on_a_terminal_table_the_note_names_the_revision_to_restore() -> TestResult {
    let mut env = Env::new()?;
    env.get("/styling", &styling_read(), 2);
    let body = styling_body(&json!({"spacing": "compact"}));
    env.server
        .mock("PUT", "/styling")
        .with_body(saved("styling", &body).to_string())
        .expect(2)
        .create();
    let history = env
        .server
        .mock("GET", "/revisions")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("resourceType".into(), "styling".into()),
            Matcher::UrlEncoded("limit".into(), "2".into()),
        ]))
        .with_body(
            json!({"revisions": [{"revisionUuid": REVISION}, {"revisionUuid": PREVIOUS}], "nextCursor": null})
                .to_string(),
        )
        .expect(1)
        .create();
    env.cmd()?
        .args(["styling", "set", "spacing=compact", "-o", "table"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "Live now. To undo: firebird-cli revisions restore {PREVIOUS}"
        )))
        .stdout(predicate::str::contains("styling.spacing"));
    history.assert();

    // no earlier revision to name: point at the history instead
    env.server
        .mock("GET", "/revisions")
        .match_query(Matcher::Any)
        .with_body(
            json!({"revisions": [{"revisionUuid": REVISION}], "nextCursor": null}).to_string(),
        )
        .create();
    env.cmd()?
        .args(["styling", "set", "spacing=compact", "-o", "table"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Live now. Its history: firebird-cli revisions list --type styling",
        ));
    Ok(())
}

#[test]
fn social_set_sends_every_platform_and_says_what_was_stored_differently() -> TestResult {
    let mut env = Env::new()?;
    let current = json!({
        "facebook": "example", "twitter": "", "linkedin": "", "instagram": "",
        "youtube": "", "tiktok": "old", "pinterest": "",
    });
    let read = env.get("/social-links", &current, 1);
    let mut sent = current.clone();
    sent["instagram"] = json!("https://www.instagram.com/example/");
    sent["tiktok"] = json!("");
    let mut stored = sent.clone();
    stored["instagram"] = json!("example");
    let answer = saved("socialLinks", &stored);
    let put = env.write("PUT", "/social-links", &sent, 200, &answer);

    let assert = env
        .cmd()?
        .args(["social", "set", "instagram=https://www.instagram.com/example/", "tiktok="])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            r#"warning: instagram was stored as "example", not "https://www.instagram.com/example/" as sent"#,
        ));
    assert_eq!(
        serde_json::from_slice::<Value>(&assert.get_output().stdout)?,
        answer
    );
    read.assert();
    put.assert();
    Ok(())
}

#[test]
fn footer_set_keeps_the_other_fields_and_null_clears_one() -> TestResult {
    let mut env = Env::new()?;
    let current = json!({
        "logoMediaUuid": MEDIA, "secondaryLogoMediaUuid": null, "showLogo": true,
        "showSecondaryLogo": false, "fullName": "Example Realty", "address": "1 Main St",
        "phone": "555-0100", "licenseNumber": "GA-1", "isEqualHousingOpportunity": true,
        "isEqualHousingLender": false, "isRealtor": true, "overrideText": null,
        "backgroundColor": null,
    });
    env.get("/footer", &current, 1);
    let mut sent = current.clone();
    sent["phone"] = Value::Null;
    sent["isRealtor"] = json!(false);
    sent["fullName"] = json!("Example Realty Group");
    let put = env.write("PUT", "/footer", &sent, 200, &saved("footer", &sent));

    let out = env.json(&[
        "footer",
        "set",
        "phone=null",
        "isRealtor=false",
        "fullName=Example Realty Group",
    ])?;
    assert_eq!(out["revisionUuid"], REVISION);
    assert_eq!(out["footer"]["address"], "1 Main St");
    put.assert();
    Ok(())
}

#[test]
fn headers_update_replaces_one_header_and_leaves_out_what_the_save_does_not_take() -> TestResult {
    let mut env = Env::new()?;
    let path = format!("/headers/{HEADER}");
    let current = json!({
        "headerUuid": HEADER, "name": "Main", "isSiteDefault": true, "pageUuids": [ABOUT],
        "logoMediaUuid": null, "secondaryLogoMediaUuid": null, "showLogo": true,
        "showSecondaryLogo": false, "isSticky": false, "isTransparent": false,
        "isHidden": false, "backgroundColor": null, "logoPosition": "left",
    });
    env.get(&path, &current, 2);
    let sent = json!({
        "logoMediaUuid": null, "secondaryLogoMediaUuid": null, "showLogo": true,
        "showSecondaryLogo": false, "isSticky": true, "isTransparent": false,
        "isHidden": false, "backgroundColor": "primary", "logoPosition": "center",
    });
    let put = env
        .server
        .mock("PUT", path.as_str())
        .match_body(Matcher::Json(sent.clone()))
        .with_body(saved("header", &sent).to_string())
        .expect(2)
        .create();
    let args = [
        "headers",
        "update",
        HEADER,
        "isSticky=true",
        "backgroundColor=primary",
        "logoPosition=center",
    ];
    assert_eq!(env.json(&args)?["header"]["logoPosition"], "center");

    // the header's history is kept under its uuid
    let history = env
        .server
        .mock("GET", "/revisions")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("resourceType".into(), "header".into()),
            Matcher::UrlEncoded("resourceKey".into(), HEADER.into()),
        ]))
        .with_body(
            json!({"revisions": [{"revisionUuid": REVISION}, {"revisionUuid": PREVIOUS}]})
                .to_string(),
        )
        .create();
    env.cmd()?
        .args(args)
        .args(["-o", "table"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "revisions restore {PREVIOUS}"
        )));
    put.assert();
    history.assert();

    env.server
        .mock("GET", format!("/headers/{MEDIA}").as_str())
        .with_status(404)
        .with_body(r#"{"error":"Header not found"}"#)
        .create();
    env.error(&["headers", "update", MEDIA, "isSticky=true"], 4)?;
    Ok(())
}

#[test]
fn a_save_the_api_refuses_keeps_its_exit_code_and_body() -> TestResult {
    let mut env = Env::new()?;
    env.get("/styling", &styling_read(), 1);
    let refused = json!({"error": "spacing Invalid option: expected one of \"compact\"|\"default\"|\"spacious\""});
    env.write(
        "PUT",
        "/styling",
        &styling_body(&json!({"spacing": "huge"})),
        400,
        &refused,
    );
    let error = env.error(&["styling", "set", "spacing=huge"], 5)?;
    assert_eq!(error["code"], "validation_error");
    assert_eq!(error["body"], refused);
    Ok(())
}

// --- page settings -------------------------------------------------------------------

fn pages() -> Value {
    json!([
        {
            "pageUuid": ABOUT, "siteUuid": "11111111-1111-4111-8111-111111111111",
            "slug": "about", "metaTitle": "About Us", "metaDescription": "Who we are",
            "metaKeywords": "team, story", "status": "active", "pageTypeId": 3,
            "navBarUuid": null, "updatedDate": "2026-01-01T00:00:00.000Z",
        },
        {"pageUuid": HOME, "slug": "", "metaTitle": null, "metaDescription": null,
         "metaKeywords": null, "status": "active"},
    ])
}

#[test]
fn pages_update_sends_back_the_settings_it_was_not_given() -> TestResult {
    let mut env = Env::new()?;
    let listed = env.get("/pages", &pages(), 4);
    let path = format!("/pages/{ABOUT}");
    let page = json!({"pageUuid": ABOUT, "slug": "about", "metaTitle": "About the team"});

    // only the title changes: the other meta fields must travel with it,
    // or the API would clear them
    let title = env.write(
        "PUT",
        &path,
        &json!({"slug": "about", "metaTitle": "About the team", "metaDescription": "Who we are", "metaKeywords": "team, story"}),
        200,
        &page,
    );
    assert_eq!(
        env.json(&["pages", "update", "about", "--meta-title", "About the team"])?,
        page
    );
    title.assert();

    // an empty value clears a field by leaving it out; slug and status are sent as given
    let cleared = env.write(
        "PUT",
        &path,
        &json!({"slug": "about-us", "metaTitle": "About Us", "metaDescription": "Who we are", "status": "unpublished"}),
        200,
        &page,
    );
    assert_eq!(
        env.text(&[
            "pages",
            "update",
            ABOUT,
            "--slug",
            "/about-us/",
            "--meta-keywords",
            "",
            "--status",
            "unpublished",
            "-y",
            "-q",
        ])?,
        format!("{ABOUT}\n")
    );
    cleared.assert();

    // the home page keeps its empty slug and its unset fields stay out
    let home = env.write(
        "PUT",
        &format!("/pages/{HOME}"),
        &json!({"slug": "", "metaDescription": "Welcome"}),
        200,
        &json!({"pageUuid": HOME, "slug": ""}),
    );
    env.json(&["pages", "update", "/", "--meta-description", "Welcome"])?;
    home.assert();

    let conflict = json!({"error": "A page with this slug already exists"});
    env.write(
        "PUT",
        &path,
        &json!({"slug": "listings", "metaTitle": "About Us", "metaDescription": "Who we are", "metaKeywords": "team, story"}),
        409,
        &conflict,
    );
    let error = env.error(&["pages", "update", "about", "--slug", "listings"], 6)?;
    assert_eq!(error["body"], conflict);
    listed.assert();

    let nothing = env.never("PUT");
    env.error(&["pages", "update", "about"], 2)?;
    nothing.assert();
    Ok(())
}

#[test]
fn pages_create_posts_the_fields_given() -> TestResult {
    let mut env = Env::new()?;
    let page = json!({"pageUuid": ABOUT, "slug": "areas/downtown", "status": "active"});
    let created = env.write(
        "POST",
        "/pages",
        &json!({"slug": "areas/downtown", "metaTitle": "Downtown", "metaDescription": "Homes downtown"}),
        201,
        &page,
    );
    assert_eq!(
        env.json(&[
            "pages",
            "create",
            "--slug",
            "/areas/downtown/",
            "--meta-title",
            "Downtown",
            "--meta-description",
            "Homes downtown",
            "--meta-keywords",
            "",
        ])?,
        page
    );
    created.assert();

    let minimal = env.write("POST", "/pages", &json!({"slug": "contact"}), 201, &page);
    assert_eq!(
        env.text(&["pages", "create", "--slug", "contact", "-q"])?,
        format!("{ABOUT}\n")
    );
    minimal.assert();

    env.write(
        "POST",
        "/pages",
        &json!({"slug": "taken"}),
        409,
        &json!({"error": "A page with this slug already exists"}),
    );
    env.error(&["pages", "create", "--slug", "taken"], 6)?;
    env.write(
        "POST",
        "/pages",
        &json!({"slug": "second"}),
        403,
        &json!({"error": "Forbidden"}),
    );
    env.error(&["pages", "create", "--slug", "second"], 3)?;

    let nothing = env.never("POST");
    env.error(&["pages", "create", "--meta-title", "No slug"], 2)?;
    env.error(&["pages", "create", "--slug", "/"], 2)?;
    nothing.assert();
    Ok(())
}

// --- areas ---------------------------------------------------------------------------

#[test]
fn areas_create_update_and_delete_call_their_routes() -> TestResult {
    let mut env = Env::new()?;
    let area = json!({"areaUuid": MEDIA, "name": "Downtown", "routeLocation": "downtown"});
    let created = env.write(
        "POST",
        "/areas",
        &json!({"name": "Downtown", "description": "The centre", "routeLocation": "downtown", "lat": 33.75, "lon": -84.39}),
        201,
        &area,
    );
    assert_eq!(
        env.text(&[
            "areas",
            "create",
            "--name",
            "Downtown",
            "--description",
            "The centre",
            "--route-location",
            "downtown",
            "--lat",
            "33.75",
            "--lon",
            "-84.39",
            "-q",
        ])?,
        format!("{MEDIA}\n")
    );
    created.assert();

    // only the flags given are sent; null clears
    let path = format!("/areas/{MEDIA}");
    let updated = env.write(
        "PATCH",
        &path,
        &json!({"routeLocation": null, "customUrl": "https://www.example.com/downtown", "mediaUuid": HEADER}),
        200,
        &area,
    );
    assert_eq!(
        env.json(&[
            "areas",
            "update",
            MEDIA,
            "--route-location",
            "null",
            "--custom-url",
            "https://www.example.com/downtown",
            "--media-uuid",
            HEADER,
        ])?,
        area
    );
    updated.assert();

    let deleted = env
        .server
        .mock("DELETE", path.as_str())
        .with_body(r#"{"success":true}"#)
        .expect(1)
        .create();
    assert_eq!(
        env.json(&["areas", "delete", MEDIA, "-y"])?,
        json!({"success": true})
    );
    deleted.assert();

    env.server
        .mock("DELETE", format!("/areas/{HEADER}").as_str())
        .with_status(404)
        .with_body(r#"{"error":"Area not found"}"#)
        .create();
    env.error(&["areas", "delete", HEADER], 4)?;

    let no_post = env.never("POST");
    let no_patch = env.never("PATCH");
    env.error(&["areas", "create", "--description", "No name"], 2)?;
    env.error(&["areas", "update", MEDIA], 2)?;
    env.error(&["areas", "update", MEDIA, "--lat", "north"], 2)?;
    no_post.assert();
    no_patch.assert();
    Ok(())
}

// --- revisions restore -----------------------------------------------------------------

#[test]
fn revisions_restore_posts_and_prints_the_new_revision() -> TestResult {
    let mut env = Env::new()?;
    let path = format!("/revisions/{PREVIOUS}/restore");
    let answer = json!({"revision": {
        "revisionUuid": REVISION, "resourceType": "styling", "source": "revert",
        "restoredFromRevisionUuid": PREVIOUS, "message": null,
    }});
    let plain = env.write("POST", &path, &json!({}), 200, &answer);
    assert_eq!(env.json(&["revisions", "restore", PREVIOUS])?, answer);
    plain.assert();

    let noted = env.write(
        "POST",
        &path,
        &json!({"message": "Undo the colour change"}),
        200,
        &answer,
    );
    assert_eq!(
        env.text(&[
            "revisions",
            "restore",
            PREVIOUS,
            "-m",
            " Undo the colour change ",
            "-y",
            "-q"
        ])?,
        format!("{REVISION}\n")
    );
    noted.assert();
    Ok(())
}

#[test]
fn restoring_the_current_revision_is_refused_readably() -> TestResult {
    let mut env = Env::new()?;
    let refused = json!({
        "error": "This revision matches the current state, so restoring it would change nothing",
        "code": "REVISION_ALREADY_CURRENT",
    });
    env.server
        .mock("POST", format!("/revisions/{REVISION}/restore").as_str())
        .with_status(422)
        .with_body(refused.to_string())
        .expect(2)
        .create();

    let error = env.error(&["revisions", "restore", REVISION], 5)?;
    assert_eq!(error["code"], "REVISION_ALREADY_CURRENT");
    assert_eq!(error["status"], 422);
    assert_eq!(error["body"], refused);
    let hint = error["hint"].as_str().unwrap_or_default();
    assert!(hint.contains("restore the revision before it"), "{hint}");

    env.cmd()?
        .args(["revisions", "restore", REVISION, "-o", "table"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains(
            "error: REVISION_ALREADY_CURRENT: This revision matches the current state",
        ))
        .stderr(predicate::str::contains(
            "hint: that revision is the setting's current state",
        ));

    // another reason keeps the API's code and message, with no made-up hint
    env.server
        .mock("POST", format!("/revisions/{PREVIOUS}/restore").as_str())
        .with_status(422)
        .with_body(r#"{"error":"A linked page no longer exists","code":"REVISION_TARGET_MISSING"}"#)
        .create();
    let error = env.error(&["revisions", "restore", PREVIOUS], 5)?;
    assert_eq!(error["code"], "REVISION_TARGET_MISSING");
    assert_eq!(error.get("hint"), None);
    Ok(())
}

// --- media -----------------------------------------------------------------------------

#[test]
fn media_import_and_update_send_only_what_was_given() -> TestResult {
    let mut env = Env::new()?;
    let media = json!({"mediaUuid": MEDIA, "filename": "porch.jpg", "mimeType": "image/jpeg"});
    let imported = env.write(
        "POST",
        "/media/import",
        &json!({"url": "https://images.example.com/porch.jpg", "alt": "A front porch"}),
        200,
        &media,
    );
    assert_eq!(
        env.text(&[
            "media",
            "import",
            "https://images.example.com/porch.jpg",
            "--alt",
            "A front porch",
            "-q",
        ])?,
        format!("{MEDIA}\n")
    );
    imported.assert();
    let bare = env.write(
        "POST",
        "/media/import",
        &json!({"url": "https://images.example.com/a.png"}),
        200,
        &media,
    );
    assert_eq!(
        env.json(&["media", "import", "https://images.example.com/a.png"])?,
        media
    );
    bare.assert();

    let path = format!("/media/{MEDIA}");
    let updated = env.write(
        "PATCH",
        &path,
        &json!({"alt": "A front porch at dusk", "tags": ["exterior", "porch"], "assetRole": "property"}),
        200,
        &media,
    );
    assert_eq!(
        env.json(&[
            "media",
            "update",
            MEDIA,
            "--alt",
            "A front porch at dusk",
            "--tags",
            "exterior, porch",
            "--asset-role",
            "property",
        ])?,
        media
    );
    updated.assert();
    let cleared = env.write(
        "PATCH",
        &path,
        &json!({"alt": null, "description": null, "tags": [], "assetRole": null}),
        200,
        &media,
    );
    env.json(&[
        "media",
        "update",
        MEDIA,
        "--alt",
        "null",
        "--description",
        "null",
        "--tags",
        "",
        "--asset-role",
        "null",
        "-y",
    ])?;
    cleared.assert();

    let nothing = env.never("PATCH");
    env.error(&["media", "update", MEDIA], 2)?;
    nothing.assert();
    Ok(())
}

/// The storage host a presigned upload goes to: another origin than the API.
struct Storage {
    server: ServerGuard,
}

impl Storage {
    fn new() -> Self {
        Self {
            server: Server::new(),
        }
    }

    fn presigned(&self) -> String {
        format!(
            "{}/bucket/site/{MEDIA}.png?X-Amz-Credential=AKIAEXAMPLE&X-Amz-Signature=5ecret51gnature",
            self.server.url()
        )
    }
}

fn upload_url(env: &mut Env, sent: &Value, presigned: &str) -> Mock {
    env.write(
        "POST",
        "/media/upload-url",
        sent,
        200,
        &json!({"presignedUrl": presigned, "mediaUuid": MEDIA, "storagePath": format!("site/{MEDIA}.png")}),
    )
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really a picture";

#[test]
fn media_upload_asks_for_an_address_puts_the_bytes_and_confirms() -> TestResult {
    let mut env = Env::new()?;
    let mut storage = Storage::new();
    let file = env.file("Front Porch.PNG", PNG)?;
    let requested = upload_url(
        &mut env,
        &json!({"filename": "Front Porch.PNG", "mimeType": "image/png", "filesize": PNG.len(), "alt": "A porch"}),
        &storage.presigned(),
    );
    // the storage host gets the bytes and their type, and no credential of ours
    let stored = storage
        .server
        .mock("PUT", format!("/bucket/site/{MEDIA}.png").as_str())
        .match_query(Matcher::UrlEncoded(
            "X-Amz-Signature".into(),
            "5ecret51gnature".into(),
        ))
        .match_header("content-type", "image/png")
        .match_header("content-length", PNG.len().to_string().as_str())
        .match_header("authorization", Matcher::Missing)
        .match_header("x-api-key", Matcher::Missing)
        .match_header("cookie", Matcher::Missing)
        .match_body(Matcher::from(PNG.to_vec()))
        .with_status(200)
        .expect(1)
        .create();
    let media = json!({"mediaUuid": MEDIA, "filename": "Front Porch.PNG", "mimeType": "image/png"});
    let confirmed = env.write(
        "POST",
        "/media/confirm",
        &json!({"mediaUuid": MEDIA}),
        200,
        &media,
    );

    let assert = env
        .cmd()?
        .args(["-v", "media", "upload"])
        .arg(&file)
        .args(["--alt", "A porch"])
        .assert()
        .success();
    let output = assert.get_output();
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout)?, media);
    requested.assert();
    stored.assert();
    confirmed.assert();

    // -v shows where the bytes went, without the signed query string
    let log = String::from_utf8_lossy(&output.stderr);
    assert!(
        log.contains(&format!(
            "> PUT {}/bucket/site/{MEDIA}.png ({} bytes)",
            storage.server.url(),
            PNG.len()
        )),
        "{log}"
    );
    for secret in [
        "5ecret51gnature",
        "AKIAEXAMPLE",
        "X-Amz",
        KEY,
        env.jwt.as_str(),
    ] {
        assert!(!log.contains(secret), "{secret} is in the log: {log}");
    }
    Ok(())
}

#[test]
fn the_type_and_name_can_be_given_and_an_unknown_type_is_refused_first() -> TestResult {
    let mut env = Env::new()?;
    let mut storage = Storage::new();
    let file = env.file("clip.bin", b"not really a video")?;
    upload_url(
        &mut env,
        &json!({"filename": "tour.mp4", "mimeType": "video/mp4", "filesize": 18}),
        &storage.presigned(),
    );
    storage
        .server
        .mock("PUT", Matcher::Any)
        .match_query(Matcher::Any)
        .match_header("content-type", "video/mp4")
        .create();
    env.write(
        "POST",
        "/media/confirm",
        &json!({"mediaUuid": MEDIA}),
        200,
        &json!({"mediaUuid": MEDIA}),
    );
    env.cmd()?
        .args(["media", "upload"])
        .arg(&file)
        .args(["--mime-type", "video/mp4", "--filename", "tour.mp4", "-q"])
        .assert()
        .success()
        .stdout(format!("{MEDIA}\n"));

    let nothing = env.never("POST");
    let unknown = env
        .cmd()?
        .args(["media", "upload"])
        .arg(&file)
        .assert()
        .code(2);
    let message = String::from_utf8_lossy(&unknown.get_output().stderr).into_owned();
    assert!(message.contains("--mime-type"), "{message}");
    // a hidden file is never read, let alone sent
    let hidden = env.file(".env", b"SECRET=1")?;
    env.cmd()?
        .args(["media", "upload"])
        .arg(&hidden)
        .args(["--mime-type", "image/png"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("it is a hidden file"));
    let empty = env.file("empty.png", b"")?;
    env.cmd()?
        .args(["media", "upload"])
        .arg(&empty)
        .assert()
        .code(2);
    env.cmd()?
        .args(["media", "upload"])
        .arg(env.work.path().join("missing.png"))
        .assert()
        .code(1);
    nothing.assert();
    Ok(())
}

#[test]
fn a_refused_upload_is_reported_without_the_signature_and_never_confirmed() -> TestResult {
    let mut env = Env::new()?;
    let mut storage = Storage::new();
    let file = env.file("porch.png", PNG)?;
    upload_url(
        &mut env,
        &json!({"filename": "porch.png", "mimeType": "image/png", "filesize": PNG.len()}),
        &storage.presigned(),
    );
    storage
        .server
        .mock("PUT", Matcher::Any)
        .match_query(Matcher::Any)
        .with_status(403)
        .with_body("<Error><Code>SignatureDoesNotMatch</Code><Message>The signature does not match.</Message><AWSAccessKeyId>AKIAEXAMPLE</AWSAccessKeyId><SignatureProvided>5ecret51gnature</SignatureProvided></Error>")
        .create();
    let never_confirmed = env.server.mock("POST", "/media/confirm").expect(0).create();

    let assert = env
        .cmd()?
        .args(["media", "upload"])
        .arg(&file)
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("HTTP 403 Forbidden: SignatureDoesNotMatch: The signature does not match."),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "warning: the upload failed, so media {MEDIA} is left pending"
        )),
        "{stderr}"
    );
    for secret in ["5ecret51gnature", "AKIAEXAMPLE", "X-Amz"] {
        assert!(
            !stderr.contains(secret),
            "{secret} is in the output: {stderr}"
        );
    }
    assert_eq!(assert.get_output().stdout, b"");
    never_confirmed.assert();
    Ok(())
}

#[test]
fn an_upload_is_not_redirected_and_not_sent_over_plain_http() -> TestResult {
    let mut env = Env::new()?;
    let mut storage = Storage::new();
    let mut elsewhere = Server::new();
    let leaked = elsewhere.mock("PUT", Matcher::Any).expect(0).create();
    let file = env.file("porch.png", PNG)?;
    let request = json!({"filename": "porch.png", "mimeType": "image/png", "filesize": PNG.len()});

    upload_url(&mut env, &request, &storage.presigned());
    storage
        .server
        .mock("PUT", Matcher::Any)
        .match_query(Matcher::Any)
        .with_status(307)
        .with_header("location", &format!("{}/bucket/x.png", elsewhere.url()))
        .create();
    env.cmd()?
        .args(["media", "upload"])
        .arg(&file)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("HTTP 307"));
    leaked.assert();

    // an address that is neither https nor a local dev host gets nothing
    upload_url(
        &mut env,
        &request,
        "http://storage.example.com/bucket/x.png?X-Amz-Signature=5ecret51gnature",
    );
    env.cmd()?
        .args(["media", "upload"])
        .arg(&file)
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "refusing to upload to http://storage.example.com/bucket/x.png: uploads go over https",
        ))
        .stderr(predicate::str::contains("5ecret51gnature").not());
    Ok(())
}

#[test]
fn a_failed_confirmation_keeps_its_exit_code_and_names_the_pending_media() -> TestResult {
    let mut env = Env::new()?;
    let mut storage = Storage::new();
    let file = env.file("porch.png", PNG)?;
    upload_url(
        &mut env,
        &json!({"filename": "porch.png", "mimeType": "image/png", "filesize": PNG.len()}),
        &storage.presigned(),
    );
    storage
        .server
        .mock("PUT", Matcher::Any)
        .match_query(Matcher::Any)
        .create();
    env.write(
        "POST",
        "/media/confirm",
        &json!({"mediaUuid": MEDIA}),
        409,
        &json!({"error": "Upload does not match the declared size"}),
    );
    env.cmd()?
        .args(["media", "upload"])
        .arg(&file)
        .assert()
        .code(6)
        .stderr(predicate::str::contains(format!(
            "warning: confirming the upload failed, so media {MEDIA} is left pending"
        )))
        .stderr(predicate::str::contains(r#""code":"conflict""#));

    // the API refusing the type or size is the API's error, before any upload
    env.write(
        "POST",
        "/media/upload-url",
        &json!({"filename": "porch.png", "mimeType": "image/tiff", "filesize": PNG.len()}),
        400,
        &json!({"error": "Unsupported media type: image/tiff"}),
    );
    let error = env.error(
        &[
            "media",
            "upload",
            &file.display().to_string(),
            "--mime-type",
            "image/tiff",
        ],
        5,
    )?;
    assert_eq!(error["message"], "Unsupported media type: image/tiff");
    Ok(())
}
