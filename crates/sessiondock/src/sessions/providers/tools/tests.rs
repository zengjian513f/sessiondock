use super::*;

#[test]
fn summaries_keep_semantic_commands_and_order() {
    for command in [
        json!("/usr/bin/zsh -lc 'rg test src'"),
        json!(["zsh", "-lc", "rg test src"]),
    ] {
        assert_eq!(
            summary("functions.exec_command", &json!({"command": command})).as_deref(),
            Some("$ rg test src")
        );
    }
    assert_eq!(summary("functions.exec", &json!(r#"text(await tools.exec_command({cmd: "one"})); text(await tools.exec_command({"cmd": "two"}));"#)).as_deref(), Some("$ one …(+1)"));
    assert_eq!(
        summary(
            "functions.exec",
            &json!(r#"tools.update_plan({plan:[{step:"read"},{step:"test"}]})"#)
        )
        .as_deref(),
        Some("计划 ×2: read; test")
    );
    assert_eq!(
        summary(
            "functions.exec",
            &json!("tools.one({}); tools.two({}); tools.one({});")
        )
        .as_deref(),
        Some("tools.one tools.two")
    );
    let args: Value =
        serde_json::from_str(r#"{"z":"first","a":false,"r":4,"b":"last","extra":"omitted"}"#)
            .unwrap();
    assert_eq!(
        summary("unknown", &args).as_deref(),
        Some("z=first a=False r=4 b=last")
    );
}

#[test]
fn summary_known_tools_and_unicode_clip() {
    assert_eq!(
        summary("Read", &json!({"file_path":"src/x.rs", "offset":5})).as_deref(),
        Some("读 src/x.rs ⌖5")
    );
    assert_eq!(
        summary("Read", &json!({"path":"x", "limit":4})).as_deref(),
        Some("读 x ⌖0+4")
    );
    assert_eq!(
        summary("Grep", &json!({"pattern":"line", "glob":"*.rs"})).as_deref(),
        Some("搜 line ⌁ *.rs")
    );
    assert_eq!(
        summary(
            "Agent",
            &json!({"description":"审阅", "subagent_type":"review"})
        )
        .as_deref(),
        Some("子代理(review): 审阅")
    );
    assert_eq!(
        summary(
            "TodoWrite",
            &json!({"todos":[{"content":"one"},null,{"subject":"two"}]})
        )
        .as_deref(),
        Some("TODO ×2: one; two")
    );
    assert_eq!(summary("unknown", &json!({"nested":{"a":1}})), None);
    assert_eq!(clip(&"文".repeat(201), 200).chars().count(), 201);
}

#[test]
fn question_normalizes_single_row_and_options() {
    let (text, questions) = question(
        "functions.request_user_input",
        &json!({"questions":{
            "header":"选择", "question":"Which?", "multiple":true,
            "options":["A",{"label":"B","description":"why"},null,{"label":""}]
        }}),
    )
    .unwrap();
    assert_eq!(text, "Which?");
    assert_eq!(
        questions,
        vec![
            json!({"header":"选择","question":"Which?","multiple":true,"options":[{"label":"A","description":""},{"label":"B","description":"why"}]})
        ]
    );
    assert!(
        question(
            "AskUserQuestion",
            &json!({"questions":[null,{}, {"question":""}]})
        )
        .is_none()
    );
}

#[test]
fn patch_changes_preserve_unknown_file_sides_and_moves() {
    let patch = "*** Begin Patch\n*** Add File: new.rs\n+new\n*** Update File: old.rs\n*** Move to: moved.rs\n@@\n-old\n+new\n*** Delete File: gone.rs\n-old\n*** End Patch";
    let changes = changes(
        "functions.exec",
        &json!(format!(
            "tools.apply_patch({});",
            serde_json::to_string(patch).unwrap()
        )),
    )
    .unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(
        changes[0],
        json!({"path":"new.rs","operation":"add","patch":"+new","added":1,"removed":0,"before_available":false,"after_available":true,"before_complete":false,"after_complete":true})
    );
    assert_eq!(changes[1]["new_path"], "moved.rs");
    assert_eq!(changes[1]["before_complete"], false);
    assert_eq!(changes[2]["after_available"], false);
    assert_eq!(changes[2]["before_complete"], true);
}

#[test]
fn unified_diff_empty_equal_replace_insert_and_delete() {
    for (old, new, expected) in [
        ("same", "same", ""),
        ("", "new\n", "--- x\n+++ x\n@@ -0,0 +1 @@\n+new"),
        ("old\n", "", "--- x\n+++ x\n@@ -1 +0,0 @@\n-old"),
        (
            "a\nb\nc",
            "a\nx\nc",
            "--- x\n+++ x\n@@ -1,3 +1,3 @@\n a\n-b\n+x\n c",
        ),
    ] {
        assert_eq!(
            edit_change("x", &json!(old), &json!(new)).unwrap()["patch"],
            expected
        );
    }
    let old = (0..20)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let new = old
        .replacen("0\n", "zero\n", 1)
        .replace("\n19", "\nnineteen");
    let change = edit_change("x", &json!(old), &json!(new)).unwrap();
    assert_eq!(change["added"], 2);
    assert_eq!(change["removed"], 2);
    assert_eq!(change["patch"].as_str().unwrap().matches("@@ -").count(), 2);
}

#[test]
fn repeated_lines_and_autojunk_follow_sequence_matcher() {
    assert_eq!(
        unified("x", &["a", "b", "a"], &["b", "a", "b"])
            .unwrap()
            .join("\n"),
        "--- x\n+++ x\n@@ -1,3 +1,3 @@\n+b\n a\n b\n-a"
    );
    let old = vec!["same"; 220];
    let mut new = old.clone();
    new[110] = "changed";
    let rows = unified("x", &old, &new).unwrap();
    // difflib's default autojunk intentionally does not anchor the popular tail.
    assert_eq!(
        rows.iter().filter(|line| line.as_str() == "-same").count(),
        110
    );
    assert_eq!(
        rows.iter().filter(|line| line.as_str() == "+same").count(),
        109
    );
}

#[test]
fn writes_and_multiedits_do_not_claim_complete_before_images() {
    let writes = changes(
        "Write",
        &json!({"file_path":"x","content":"a\r\nb\u{2028}"}),
    )
    .unwrap();
    assert_eq!(writes[0]["patch"], "+a\n+b");
    assert_eq!(writes[0]["before_available"], false);
    assert_eq!(writes[0]["after_complete"], true);
    let edits = changes(
        "MultiEdit",
        &json!({"path":"x","edits":[{"old_string":"a","new_string":"b"},null,{}]}),
    )
    .unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0]["before_complete"], false);
    assert_eq!(edits[1]["patch"], "");
}

#[test]
fn huge_changes_return_explicit_presentation_limit() {
    assert!(
        changes(
            "Write",
            &json!({"path":"x","content":"x".repeat(DIFF_BYTES+1)})
        )
        .unwrap_err()
        .contains("512 KiB")
    );
    assert!(
        changes(
            "Edit",
            &json!({"path":"x","old_string":"a\n".repeat(DIFF_LINES+1),"new_string":""})
        )
        .unwrap_err()
        .contains("20000")
    );
}

#[test]
fn output_envelopes_and_python_stringify_fallbacks_can_be_one_of_multiple_parts() {
    let envelope =
        json!({"output":"actual stdout","exit_code":1,"wall_time_seconds":0.25}).to_string();
    let (text, fields) = output(&json!([{"type":"text", "text":format!("Script completed\nOutput:\n{envelope}")}, {"type":"text", "text":"wrapper tail"}])).unwrap();
    assert_eq!(text, "actual stdout");
    assert_eq!(fields, json!({"exit_code":1,"duration_s":0.25}));
    let business = json!({"output":"business data"});
    assert_eq!(output(&business).unwrap(), (string(&business), json!({})));
    assert_eq!(
        output(&json!({"text":"plain text"})).unwrap().0,
        "plain text"
    );
    assert_eq!(
        output(&json!([{"type":"image", "source":{}}])).unwrap().0,
        "[图片]"
    );
    assert_eq!(
        output(&json!([{"type":"unknown", "text":"rendered text"}]))
            .unwrap()
            .0,
        "rendered text"
    );
    let unknown = json!({"type":"future", "payload":{"value":1}});
    assert_eq!(output(&unknown).unwrap().0, string(&unknown));
    assert_eq!(
        output(&json!(format!("preface inline {envelope}")))
            .unwrap()
            .1,
        json!({})
    );
}

const HEADER: &str = "Script completed\nWall time 0.2 seconds\nOutput:\n";

fn chunk(id: &str, output: &str, wall: f64, exit: Option<i64>) -> Value {
    let mut envelope = json!({"chunk_id": id, "wall_time_seconds": wall,
        "original_token_count": 7, "output": output});
    match exit {
        Some(code) => envelope["exit_code"] = json!(code),
        None => envelope["session_id"] = json!(4242),
    }
    envelope
}

fn part(text: impl std::fmt::Display) -> Value {
    json!({"type": "input_text", "text": text.to_string()})
}

/// Every chunk of a multi-part `exec` result is shown, in
/// part order, as the recorded concatenation; `exit_code` is the last chunk's
/// and `duration_s` the sum. The reference adapter shows one chunk only.
#[test]
fn multi_part_outputs_concatenate_every_chunk_in_order() {
    for count in [2usize, 3, 5] {
        let chunks = (0..count)
            .map(|index| {
                chunk(
                    &format!("c{index}"),
                    &format!("line {index}\n"),
                    0.5,
                    Some(0),
                )
            })
            .collect::<Vec<_>>();
        let mut parts = vec![part(HEADER)];
        parts.extend(chunks.iter().map(part));
        let (text, fields) = output(&Value::Array(parts)).unwrap();
        assert_eq!(
            text,
            (0..count)
                .map(|index| format!("line {index}\n"))
                .collect::<String>(),
            "{count} chunks"
        );
        assert_eq!(fields["exit_code"], 0);
        assert_eq!(fields["duration_s"].as_f64().unwrap(), 0.5 * count as f64);
        assert!(fields.get("error").is_none());
    }
}

#[test]
fn multi_part_exit_code_comes_from_the_last_chunk_that_has_one() {
    let parts = json!([
        part(HEADER),
        part(chunk("a", "first", 1.0, Some(0))),
        part(chunk("b", " second", 2.0, None)),
        part(chunk("c", " third\n", 0.25, Some(3))),
        part(chunk("d", "", 1.0, None)),
    ]);
    let (text, fields) = output(&parts).unwrap();
    assert_eq!(text, "first second third\n");
    assert_eq!(fields, json!({"exit_code": 3, "duration_s": 4.25}));
    // No chunk carries an exit code: the field is absent, never invented.
    let (_, fields) = output(&json!([
        part(HEADER),
        part(chunk("a", "x", 1.0, None)),
        part(chunk("b", "y", 1.0, None))
    ]))
    .unwrap();
    assert_eq!(fields, json!({"duration_s": 2.0}));
}

/// The header, an empty trailing part, the script's own prints (`--- 1 ---`,
/// `{}`, `[]`, status objects) and a plain-text part are not chunks and never
/// enter the text; a structural chunk (decoded giant part) is a chunk like
/// any other.
#[test]
fn multi_part_non_chunk_parts_are_dropped_and_structural_chunks_count() {
    let parts = json!([
        part(HEADER),
        part("--- 1 ---"),
        part(chunk("a", "alpha\n", 0.5, Some(0))),
        part("{}"),
        part("[]"),
        part(json!({"i": 0, "status": "fulfilled"})),
        json!({"type": "input_text", "text": "web search result\n"}),
        chunk("b", "beta\n", 0.5, Some(1)),
        part("\u{2003}".to_owned() + &chunk("c", "gamma\n", 1.0, Some(0)).to_string() + "\u{a0}\n"),
        part(""),
    ]);
    let (text, fields) = output(&parts).unwrap();
    assert_eq!(text, "alpha\nbeta\ngamma\n");
    assert_eq!(fields, json!({"exit_code": 0, "duration_s": 2.0}));
    // A wrapped `{"i","status","value":{envelope}}` part is not a chunk: the
    // reference adapter shows such outputs raw and so does this parser.
    let wrapped = json!([
        part(HEADER),
        part(json!({"i": 0, "status": "fulfilled", "value": chunk("a", "alpha", 0.5, Some(0))})),
        part(json!({"i": 1, "status": "fulfilled", "value": chunk("b", "beta", 0.5, Some(0))})),
    ]);
    let (text, fields) = output(&wrapped).unwrap();
    assert!(text.starts_with(HEADER) && text.contains("\"status\":\"fulfilled\""));
    assert_eq!(fields, json!({}));
}

/// One chunk keeps the single-envelope result exactly (parity with the
/// reference adapter), whatever else surrounds it.
#[test]
fn single_chunk_parts_match_the_single_envelope_search() {
    let single = chunk("a", "only\n", 0.75, Some(2));
    for parts in [
        json!([part(HEADER), part(&single)]),
        json!([part(HEADER), part(&single), part("SESSION_ID=10756")]),
        json!([part(HEADER), part("plain web result"), part(&single)]),
        json!([part(HEADER), part(&single), part("")]),
        json!([
            part(HEADER),
            part(json!({"i": 0, "status": "fulfilled"})),
            part(&single)
        ]),
        json!([part(HEADER), single.clone()]),
    ] {
        assert_eq!(
            output(&parts).unwrap(),
            (
                "only\n".to_owned(),
                json!({"exit_code": 2, "duration_s": 0.75})
            ),
            "{parts}"
        );
    }
    // A prefixed envelope inside one part is not a whole-part chunk; the
    // batch-19 search still finds it.
    let prefixed = json!([part(format!("wait…\nOutput:\n{single}")), part("tail")]);
    assert_eq!(output(&prefixed).unwrap().0, "only\n");
}

#[test]
fn multi_part_media_and_error_flags_follow_the_chunks() {
    let image = json!({"type":"input_image","image_url":{"url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="}});
    let failing = json!({"chunk_id":"m","wall_time_seconds":0.5,"exit_code":0,
        "output":{"content":[{"type":"text","text":"mcp text"}, image.clone()],"isError":true}});
    let parts = json!([
        part(HEADER),
        part(chunk("a", "alpha\n", 0.5, Some(0))),
        part(failing),
        image,
        part(""),
    ]);
    let (cleaned, media) = sanitize_output_with_media(&parts, None).unwrap();
    assert_eq!(media.len(), 2, "trailing image part plus the MCP image");
    assert!(!cleaned.to_string().contains("base64,"));
    assert_eq!(
        cleaned.as_array().unwrap().len(),
        2,
        "only the chunks remain"
    );
    let (text, fields) = output(&cleaned).unwrap();
    assert_eq!(text, "alpha\nmcp text");
    assert_eq!(
        fields,
        json!({"exit_code": 0, "duration_s": 1.0, "error": true})
    );
    // Extra serializable block kinds do not invalidate streamed envelopes.
    let unknown = json!([part(HEADER), part(chunk("a", "x", 1.0, Some(0))),
        part(chunk("b", "y", 1.0, Some(0))), {"type":"unknown","text":"not trusted"}]);
    assert_eq!(output(&unknown).unwrap().0, "xy");
}

#[test]
fn question_answers_handle_cancel_boundary_and_empty_rows() {
    assert_eq!(
        answer(&json!(" Aborted by user with explanation ")).unwrap(),
        ("已取消回答".to_owned(), true)
    );
    assert_eq!(
        answer(&json!("Aborted by username")).unwrap(),
        ("Aborted by username".to_owned(), false)
    );
    let value =
        json!({"answers":{"one":{"answers":["A", " ", "B"]},"two":{"answers":[]},"three":" C "}});
    assert_eq!(answer(&value).unwrap(), ("A、B\nC".to_owned(), false));
    let empty = json!({"answers":{"one":{"answers":[]}}});
    assert_eq!(answer(&empty).unwrap().0, string(&empty));
}

#[test]
fn output_envelopes_have_no_candidate_count_quota() {
    let raw = format!(
        "{}{}",
        "{invalid}\n".repeat(40),
        json!({"output":"FOUND","wall_time_seconds":1,"exit_code":0})
    );
    assert_eq!(output(&json!(raw)).unwrap().0, "FOUND");
    let mut parts = vec![json!("ordinary"); 70];
    parts.push(json!(format!(
        "prefix\nOutput:\n{}",
        json!({"output":"LATE","wall_time_seconds":1,"exit_code":0})
    )));
    parts.push(json!("trailing ordinary"));
    assert_eq!(output(&json!(parts)).unwrap().0, "LATE");
}
