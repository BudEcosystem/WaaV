//! What the OpenAI-protocol clones (Inworld, Yandex) take in a `session.update` beyond
//! OpenAI's own session: each clone's provider fixes edit the session as JSON and send it as
//! [`ClientEvent::Raw`], so OpenAI's typed session is untouched.

use serde_json::Value;

use super::ClientEvent;

/// Edit the `session` of each `session.update` in `wires` (now sent as raw JSON). Other
/// wires and an unchanged session stay as they are.
pub(crate) fn edit_session(wires: &mut [ClientEvent], edit: impl Fn(&mut Value)) {
    for wire in wires {
        if let ClientEvent::SessionUpdate { .. } = wire
            && let Ok(mut event) = serde_json::to_value(&*wire)
        {
            let before = event.clone();
            edit(&mut event["session"]);
            if event != before {
                *wire = ClientEvent::Raw(event);
            }
        }
    }
}

/// Provider fix `openai_clone_voice`: the clone's own voice name, as given (Inworld's
/// `Dennis`, Yandex SpeechKit's `dasha`), where the OpenAI session coerces any name outside
/// OpenAI's voices to `alloy`. No voice named: none is sent, so the vendor's default applies.
pub(crate) fn set_voice(session: &mut Value, voice: Option<&str>) {
    let Some(output) = session
        .get_mut("audio")
        .and_then(|a| a.get_mut("output"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    match voice.map(str::trim).filter(|v| !v.is_empty()) {
        Some(voice) => {
            output.insert("voice".into(), voice.into());
        }
        None => {
            output.remove("voice");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The voice is set as given (trimmed) or removed; other wires are left alone.
    #[test]
    fn set_voice_sets_or_removes_the_voice_and_other_wires_stay() {
        let mut v =
            json!({"audio": {"output": {"voice": "alloy", "format": {"type": "audio/pcm"}}}});
        set_voice(&mut v, None);
        assert_eq!(
            v,
            json!({"audio": {"output": {"format": {"type": "audio/pcm"}}}})
        );
        set_voice(&mut v, Some(" Clive "));
        assert_eq!(v["audio"]["output"]["voice"], "Clive");
        let mut wires = vec![ClientEvent::ResponseCancel];
        edit_session(&mut wires, |s| set_voice(s, Some("dasha")));
        assert!(matches!(wires[0], ClientEvent::ResponseCancel));
    }
}
