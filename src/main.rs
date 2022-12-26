use std::env;

use dotenv::dotenv;
use lazy_static::lazy_static;
use matrix_sdk::{
    config::SyncSettings,
    event_handler::Ctx,
    room::Room,
    ruma::{
        events::room::message::{
            MessageType, OriginalSyncRoomMessageEvent, RoomMessageEventContent,
        },
        RoomId,
    },
    Client,
};
use regex::Regex;
use rspotify::model::{FullTrack, TrackId};
use spotify::SpotifyClient;

mod formatted;
mod spotify;

lazy_static! {
    static ref RX_TRACK_URL: Regex =
        Regex::new(r"https://open.spotify.com/track/([^\?]+)").unwrap();
}

async fn get_queue_handler(spotify: &SpotifyClient) -> anyhow::Result<RoomMessageEventContent> {
    let tracks = spotify
        .get_queue()
        .await?
        .iter()
        .map(|x| formatted::track(x))
        .fold(String::new(), |a, b| a + &b + "\n");

    Ok(RoomMessageEventContent::text_markdown(format!(
        "```\n{}\n```",
        tracks.trim_end()
    )))
}

async fn get_track_handler(
    spotify: &SpotifyClient,
    id: &str,
) -> anyhow::Result<RoomMessageEventContent> {
    match spotify
        .get_track(TrackId::from_id_or_uri(id).unwrap())
        .await
    {
        Ok(track) => queue_track(&spotify, &track).await,
        Err(e) => Err(e),
    }
}

async fn search_track_handler(
    spotify: &SpotifyClient,
    search_term: &str,
) -> anyhow::Result<RoomMessageEventContent> {
    match spotify.search_track(search_term).await {
        Ok(Some(track)) => queue_track(spotify, &track).await,
        Ok(None) => Ok(RoomMessageEventContent::text_plain(format!(
            "No tracks found matching: \"{}\"",
            search_term
        ))),
        Err(e) => Err(e),
    }
}

async fn queue_track(
    spotify: &SpotifyClient,
    track: &FullTrack,
) -> anyhow::Result<RoomMessageEventContent> {
    spotify.queue_track(track).await?;
    Ok(RoomMessageEventContent::text_plain(format!(
        "Queued: {}",
        formatted::track(track)
    )))
}

async fn on_room_message(
    event: OriginalSyncRoomMessageEvent,
    room: Room,
    spotify: Ctx<SpotifyClient>,
) {
    if let Room::Joined(room) = room {
        let MessageType::Text(message) = event.content.msgtype else {
            return;
        };

        if !message.body.starts_with("!q") {
            return;
        }

        let response = if message.body == "!q" {
            Some(get_queue_handler(&spotify).await)
        } else if let Some(c) = RX_TRACK_URL.captures(&message.body) {
            Some(get_track_handler(&spotify, c.get(1).map(|x| x.as_str()).unwrap()).await)
        } else if let Some(search_term) = message.body.get(3..) {
            Some(search_track_handler(&spotify, search_term).await)
        } else {
            None
        };

        match response {
            Some(Ok(message)) => {
                room.send(message, None).await.unwrap();
            }
            Some(Err(e)) => {
                room.send(formatted::error(e).await, None).await.unwrap();
            }
            None => (),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let homeserver_url = env::var("MATRIX_HOMESERVER_URL").expect("MATRIX_HOMESERVER_URL not set");
    let username = env::var("MATRIX_USERNAME").expect("MATRIX_USERNAME not set");
    let password = env::var("MATRIX_PASSWORD").expect("MATRIX_PASSWORD not set");
    let room_id = env::var("MATRIX_ROOM_ID").expect("MATRIX_ROOM_ID not set");
    let room_id: &str = &room_id;
    let spotify = spotify::login().await?;

    let client = Client::builder()
        .homeserver_url(homeserver_url)
        .build()
        .await?;

    client.login_username(&username, &password).send().await?;

    let room_id = <&RoomId>::try_from(room_id).unwrap();
    let response = client.sync_once(SyncSettings::default()).await?;
    client.add_event_handler_context(spotify);
    client.add_room_event_handler(room_id, on_room_message);
    client.join_room_by_id(room_id).await?;
    println!("Joined {}", room_id);

    let settings = SyncSettings::default().token(response.next_batch);
    client.sync(settings).await?;

    Ok(())
}
