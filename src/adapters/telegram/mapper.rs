//! Map Grammers types to domain entities.
//!
//! Extracts Chat, Message, MediaReference from grammers_client tl types.

use crate::domain::{Chat, ChatType, MediaReference, MediaType, Message};
use grammers_client::peer::Peer;
use grammers_client::tl;

/// Map a grammers Peer to domain ChatType.
///
/// * `Peer::User` → Private (DM).
/// * `Peer::Group` → Group or Supergroup (Supergroup when megagroup).
/// * `Peer::Channel` → Channel (broadcast).
pub fn chat_type_from_peer(peer: &Peer) -> ChatType {
    match peer {
        Peer::User(_) => ChatType::Private,
        Peer::Group(g) => {
            if g.is_megagroup() {
                ChatType::Supergroup
            } else {
                ChatType::Group
            }
        }
        Peer::Channel(_) => ChatType::Channel,
    }
}

/// Map a grammers Dialog/PeerRef to domain Chat (used when building Chat in client).
/// `top_message_id`: from dialog's last message; used as heuristic for approx_message_count.
#[allow(dead_code)]
pub fn dialog_to_chat_from_ref(
    id: i64,
    name: &str,
    username: Option<&str>,
    kind: ChatType,
) -> Chat {
    dialog_to_chat(id, name, username, kind, None)
}

/// Build domain Chat with optional approximate message count (from dialog top/last message ID).
pub fn dialog_to_chat(
    id: i64,
    name: &str,
    username: Option<&str>,
    kind: ChatType,
    approx_message_count: Option<i32>,
) -> Chat {
    Chat {
        id,
        title: name.to_string(),
        username: username.map(String::from),
        kind,
        approx_message_count,
    }
}

/// Map grammers Message to domain Message. Extracts media ref for pipeline.
pub fn message_to_domain(
    msg: &tl::enums::Message,
    chat_id: i64,
) -> Option<(Message, Option<MediaReference>)> {
    let (id, date, text, from_user_id, reply_to, media_ref) = match msg {
        tl::enums::Message::Empty(_) => return None,
        tl::enums::Message::Message(m) => {
            let text = m.message.clone();
            let from = m.from_id.as_ref().and_then(|f| match f {
                tl::enums::Peer::User(u) => Some(u.user_id as i64),
                _ => None,
            });
            let media_ref: Option<MediaReference> = extract_media_ref(m, chat_id);
            (
                m.id,
                // Prefer edit_date when present so the "current" version has the edit timestamp.
                m.edit_date.map(|d| d as i64).unwrap_or(m.date as i64),
                text,
                from,
                m.reply_to
                    .as_ref()
                    .and_then(|r| match r {
                        tl::enums::MessageReplyHeader::Header(h) => Some(h.reply_to_msg_id),
                        _ => None,
                    })
                    .flatten(),
                media_ref,
            )
        }
        tl::enums::Message::Service(_) => return None,
    };

    Some((
        Message {
            id,
            chat_id,
            date,
            text,
            media: media_ref.clone(),
            from_user_id,
            reply_to_msg_id: reply_to,
            edit_history: None,
        },
        media_ref,
    ))
}

fn extract_media_ref(m: &tl::types::Message, chat_id: i64) -> Option<MediaReference> {
    let media = m.media.as_ref()?;
    let (media_type, opaque) = match media {
        tl::enums::MessageMedia::Photo(_) => (MediaType::Photo, format!("{}:{}", chat_id, m.id)),
        tl::enums::MessageMedia::Document(d) => {
            let mt = match d.document.as_ref() {
                Some(tl::enums::Document::Document(doc)) => {
                    if doc.mime_type.starts_with("video/") {
                        MediaType::Video
                    } else if doc.mime_type.starts_with("audio/") {
                        MediaType::Audio
                    } else if doc.mime_type == "application/x-tgsticker" {
                        MediaType::Sticker
                    } else {
                        MediaType::Document
                    }
                }
                _ => MediaType::Document,
            };
            (mt, format!("{}:{}", chat_id, m.id))
        }
        _ => (MediaType::Other, format!("{}:{}", chat_id, m.id)),
    };
    Some(MediaReference {
        message_id: m.id,
        chat_id,
        media_type,
        opaque_ref: opaque,
    })
}
