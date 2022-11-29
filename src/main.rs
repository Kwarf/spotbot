use std::env;

use dotenv::dotenv;
use matrix_sdk::{
    config::SyncSettings,
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
use rspotify::{
    clients::OAuthClient,
    model::{FullTrack, Market, PlayableItem, SearchResult, SearchType, TrackId},
    prelude::{BaseClient, PlayableId::Track},
    scopes, AuthCodeSpotify, ClientError, Config, Credentials, OAuth,
};

fn fmt_track(track: &FullTrack) -> String {
    format!(
        "{} - {}",
        track
            .artists
            .iter()
            .map(|artist| &artist.name)
            .fold(String::new(), |a, b| a + &b + ", ")
            .trim_end_matches(", "),
        track.name
    )
}

async fn fmt_err(error: ClientError) -> RoomMessageEventContent {
    match error {
        ClientError::Http(http) => match *http {
            rspotify::http::HttpError::StatusCode(error) => {
                RoomMessageEventContent::text_markdown(format!(
                    "```\n{}\n{}\n```",
                    error.status(),
                    error.text().await.unwrap()
                ))
            }
            rspotify::http::HttpError::Client(_) => todo!(),
        },
        _ => RoomMessageEventContent::text_plain(error.to_string()),
    }
}

async fn on_room_message(
    event: OriginalSyncRoomMessageEvent,
    room: Room,
    spotify: AuthCodeSpotify,
) {
    match room {
        Room::Joined(room) => {
            let MessageType::Text(text_content) = event.content.msgtype else {
                return;
            };

            if text_content.body == "!q" {
                println!(
                    "{} requested the queue, sending...",
                    event.sender.localpart()
                );
                room.send(
                    match spotify.current_user_queue().await {
                        Ok(queue) => RoomMessageEventContent::text_markdown(format!(
                            "```\n{}\n```",
                            queue
                                .queue
                                .iter()
                                .map(|x| match x {
                                    PlayableItem::Track(track) => fmt_track(track),
                                    PlayableItem::Episode(episode) => episode.name.clone(),
                                })
                                .fold(String::new(), |a, b| a + &b + "\n")
                                .trim_end()
                        )),
                        Err(err) => fmt_err(err).await,
                    },
                    None,
                )
                .await
                .unwrap();
            }

            if !text_content.body.starts_with("!q ") {
                return;
            }

            if let Some(args) = text_content.body.get(3..) {
                println!("{} is searching for {}...", event.sender.localpart(), args);

                let re = Regex::new(r"https://open.spotify.com/track/([^\?]+)").unwrap();
                if let Some(c) = re.captures(args) {
                    if let Ok(id) =
                        TrackId::from_id_or_uri(c.get(1).map(|x| x.as_str()).unwrap_or(""))
                    {
                        room.send(
                            match spotify.add_item_to_queue(Track(id.clone()), None).await {
                                Ok(_) => {
                                    RoomMessageEventContent::text_plain(format!("Queued: {}", id))
                                }
                                Err(err) => fmt_err(err).await,
                            },
                            None,
                        )
                        .await
                        .unwrap();
                        return;
                    }
                }

                room.send(
                    match spotify
                        .search(
                            args,
                            SearchType::Track,
                            Some(Market::FromToken),
                            None,
                            Some(1),
                            None,
                        )
                        .await
                    {
                        Ok(result) => match result {
                            SearchResult::Tracks(tracks) => match tracks.items.len() {
                                0 => RoomMessageEventContent::text_plain(format!(
                                    "No tracks found matching: \"{}\"",
                                    args
                                )),
                                _ => {
                                    let track = &tracks.items[0];
                                    match spotify
                                        .add_item_to_queue(Track(track.id.clone().unwrap()), None)
                                        .await
                                    {
                                        Ok(_) => RoomMessageEventContent::text_plain(format!(
                                            "Queued: {}",
                                            fmt_track(track)
                                        )),
                                        Err(err) => fmt_err(err).await,
                                    }
                                }
                            },
                            _ => RoomMessageEventContent::text_plain(format!(
                                "No tracks found matching: \"{}\"",
                                args
                            )),
                        },
                        Err(err) => fmt_err(err).await,
                    },
                    None,
                )
                .await
                .unwrap();
            }
        }
        _ => {}
    }
}

async fn spotify_login() -> anyhow::Result<AuthCodeSpotify> {
    let creds = Credentials::from_env().unwrap();
    let oauth = OAuth::from_env(scopes!(
        "user-read-playback-state",
        "user-modify-playback-state",
        "user-read-currently-playing"
    ))
    .unwrap();
    let config = Config {
        token_cached: true,
        token_refreshing: true,
        ..Default::default()
    };

    let spotify = AuthCodeSpotify::with_config(creds, oauth, config);
    let url = spotify.get_authorize_url(false)?;
    spotify.prompt_for_token(&url).await?;
    println!("Connected to Spotify");
    Ok(spotify)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let homeserver_url = env::var("MATRIX_HOMESERVER_URL").expect("MATRIX_HOMESERVER_URL not set");
    let username = env::var("MATRIX_USERNAME").expect("MATRIX_USERNAME not set");
    let password = env::var("MATRIX_PASSWORD").expect("MATRIX_PASSWORD not set");
    let room_id = env::var("MATRIX_ROOM_ID").expect("MATRIX_ROOM_ID not set");
    let room_id: &str = &room_id;
    let spotify = spotify_login().await?;

    let client = Client::builder()
        .homeserver_url(homeserver_url)
        .build()
        .await?;

    client.login_username(&username, &password).send().await?;

    let response = client.sync_once(SyncSettings::default()).await?;
    client.add_event_handler(move |ev, room| on_room_message(ev, room, spotify.clone()));
    client
        .join_room_by_id(<&RoomId>::try_from(room_id).unwrap())
        .await?;
    println!("Joined {}", room_id);

    let settings = SyncSettings::default().token(response.next_batch);
    client.sync(settings).await?;

    Ok(())
}
