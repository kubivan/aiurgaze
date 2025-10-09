use bevy::asset::AssetServer;
use bevy::color::Color;
use sc2_proto::sc2api::Response_oneof_response::{game_info, observation};
use bevy::prelude::{Assets, Commands, Entity, Image, Local, Query, Res, ResMut, Resource};
use bevy_ecs_tilemap::prelude::{TileColor, TileStorage};
use bevy_ecs_tilemap::tiles::TilePos;
use sc2_proto::sc2api::{Response, ResponseGameInfo, ResponseObservation};
use tokio::sync::broadcast;
use tokio::task;
use crate::proxy_ws::{ProxyWS, ProxyWSResource};
use bevy_tokio_tasks::{TokioTasksPlugin, TokioTasksRuntime};
use crate::map::{spawn_tilemap};

use std::collections::HashMap;
use crate::units::{handle_observation, UnitIconAssets, UnitRegistry};

pub fn setup_proxy(mut commands: Commands, runtime: Res<TokioTasksRuntime>) {
    println!("======setup_proxy====");
    // create proxy + channnel

    // TODO: add params
    let proxy = ProxyWS::new("127.0.0.1:5000", "ws://127.0.0.1:5555/sc2api");
    let rx = proxy.tx.subscribe();
    commands.insert_resource( ProxyWSResource { rx });
    println!("======Proxy resource inserted====");

    runtime.spawn_background_task(|_ctx| async move {
        if let Err(e) = proxy.run().await {
            eprintln!("Proxy task failed: {e}");
        }
    });
}

fn recolor_tile(
    commands: &mut Commands,
    storage: &mut TileStorage,
    pos: &TilePos,
    color: Color
) {
    if let Some(tile_entity) = storage.get(&pos) {
        commands.entity(tile_entity).insert(TileColor(color));
    }
}

pub fn response_controller_system(
    mut proxy_res: ResMut<ProxyWSResource>,
    mut commands: Commands,
    mut asset_server: Res<AssetServer>,
    mut icon_assets: Res<UnitIconAssets>,
    mut registry: ResMut<UnitRegistry>,
) {
    while let Ok(resp) = proxy_res.rx.try_recv() {
        //println!("Got response: {:?}", resp);

        match resp.response.unwrap() {
            observation (obs )  => {
                //println!("Got observation: {:?}", obs.observation.unwrap().game_loop);
                handle_observation(&mut commands,
                                   &mut asset_server,
                                   &mut icon_assets,
                                   &mut registry, &obs);
            }
            game_info (gi) =>  {
                let &start_raw = &gi.start_raw.as_ref().unwrap();
                let &start_pos = &start_raw.start_locations.get(0).unwrap();
                let path_layer = crate::map::TerrainLayer::from_image_data(
                    start_raw.placement_grid.get_ref(),
                    crate::map::TerrainLayerKind::Pathing);
                println!("Got game info: map size {} : {}", path_layer.width, path_layer.height);

                // 👇 draw the map
                let mut tile_storage = spawn_tilemap(
                    &mut commands,
                    &path_layer,
                    &mut asset_server,
                );
                let blue = Color::srgba(0.0, 0.0, 1.0, 1.0);
                let black = Color::srgba(0.0, 0.0, 0.0, 1.0);
                recolor_tile(&mut commands,
                             &mut tile_storage,
                             &TilePos { x: start_pos.x.unwrap() as u32, y: start_pos.y.unwrap() as u32 },
                             blue);
                recolor_tile(&mut commands,
                             &mut tile_storage,
                             &TilePos { x: 0, y: 0 },
                             black);

                println!("Spawned tilemap entity, start pos : {:?}", start_pos);

            }
            _ => ()
        };
    }
}