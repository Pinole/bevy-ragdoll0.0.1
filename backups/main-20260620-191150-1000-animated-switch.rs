use std::time::Duration;

use bevy::{
    asset::AssetPlugin,
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    gltf::GltfAssetLabel,
    prelude::*,
    world_serialization::WorldInstanceReady,
};

const MODEL_PATH: &str = "standingholdingjaw.glb";
const MODEL_CENTER: Vec3 = Vec3::new(0.0, 2.3, 0.18);
const INSTANCE_COUNT: usize = 1000;
const GRID_COLUMNS: usize = 40;
const INSTANCE_SPACING: f32 = 3.0;
const ANIMATION_NAMES: [&str; 2] = ["walking", "whypausegame"];

#[derive(Resource)]
struct Animations {
    graph_handle: Handle<AnimationGraph>,
    node_indices: Vec<AnimationNodeIndex>,
    current_index: usize,
}

#[derive(Resource, Default)]
struct StartedAnimationPlayers(usize);

#[derive(Component)]
struct AnimatedModelRoot;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: env!("CARGO_MANIFEST_DIR").into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "GLB model viewer".into(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins(FreeCameraPlugin)
        .insert_resource(ClearColor(Color::srgb(0.12, 0.13, 0.14)))
        .insert_resource(GlobalAmbientLight {
            brightness: 1200.0,
            ..default()
        })
        .init_resource::<StartedAnimationPlayers>()
        .add_systems(Startup, setup)
        .add_systems(Update, switch_animation)
        .run();
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut animation_graphs: ResMut<Assets<AnimationGraph>>,
) {
    let (animation_graph, node_indices) = AnimationGraph::from_clips([
        asset_server.load(GltfAssetLabel::Animation(0).from_asset(MODEL_PATH)),
        asset_server.load(GltfAssetLabel::Animation(1).from_asset(MODEL_PATH)),
    ]);

    let model_scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(MODEL_PATH));

    commands.insert_resource(Animations {
        graph_handle: animation_graphs.add(animation_graph),
        node_indices,
        current_index: 0,
    });

    let rows = INSTANCE_COUNT.div_ceil(GRID_COLUMNS);
    for index in 0..INSTANCE_COUNT {
        let column = index % GRID_COLUMNS;
        let row = index / GRID_COLUMNS;
        let x = (column as f32 - (GRID_COLUMNS as f32 - 1.0) * 0.5) * INSTANCE_SPACING;
        let z = (row as f32 - (rows as f32 - 1.0) * 0.5) * INSTANCE_SPACING;

        commands
            .spawn((
                AnimatedModelRoot,
                WorldAssetRoot(model_scene.clone()),
                Transform::from_translation(Vec3::new(x, 0.0, z)),
            ))
            .observe(play_animation_when_ready);
    }

    commands.spawn((
        Mesh3d(asset_server.add(Plane3d::default().mesh().size(130.0, 85.0).into())),
        MeshMaterial3d(asset_server.add(StandardMaterial {
            base_color: Color::srgb(0.18, 0.19, 0.2),
            perceptual_roughness: 0.85,
            ..default()
        })),
        Transform::from_translation(Vec3::ZERO),
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 42.0, 120.0).looking_at(MODEL_CENTER, Vec3::Y),
        FreeCamera {
            walk_speed: 12.0,
            run_speed: 45.0,
            ..default()
        },
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 25_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 5.0).looking_at(MODEL_CENTER, Vec3::Y),
    ));

    commands.spawn((
        PointLight {
            intensity: 4_000_000.0,
            range: 80.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(0.0, 35.0, 28.0),
    ));
}

fn play_animation_when_ready(
    scene_ready: On<WorldInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    animations: Res<Animations>,
    mut started_players: ResMut<StartedAnimationPlayers>,
    mut players: Query<&mut AnimationPlayer>,
) {
    for child in children.iter_descendants(scene_ready.entity) {
        let Ok(mut player) = players.get_mut(child) else {
            continue;
        };

        let mut transitions = AnimationTransitions::new();
        transitions
            .play(
                &mut player,
                animations.node_indices[animations.current_index],
                Duration::ZERO,
            )
            .repeat();

        commands
            .entity(child)
            .insert(AnimationGraphHandle(animations.graph_handle.clone()))
            .insert(transitions);

        started_players.0 += 1;
        if started_players.0 % 100 == 0 || started_players.0 == INSTANCE_COUNT {
            info!(
                "Started {}/{} animated model instances with '{}'",
                started_players.0, INSTANCE_COUNT, ANIMATION_NAMES[animations.current_index]
            );
        }
    }
}

fn switch_animation(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut animations: ResMut<Animations>,
    mut players: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    let next_index = if keyboard_input.just_pressed(KeyCode::Digit1) {
        Some(0)
    } else if keyboard_input.just_pressed(KeyCode::Digit2) {
        Some(1)
    } else if keyboard_input.just_pressed(KeyCode::Enter) {
        Some((animations.current_index + 1) % animations.node_indices.len())
    } else {
        None
    };

    let Some(next_index) = next_index else {
        return;
    };

    if next_index >= animations.node_indices.len() {
        return;
    }

    animations.current_index = next_index;
    let next_animation = animations.node_indices[next_index];
    let mut switched_players = 0;

    for (mut player, mut transitions) in &mut players {
        transitions
            .play(&mut player, next_animation, Duration::from_millis(250))
            .repeat();
        switched_players += 1;
    }

    info!(
        "Switched {switched_players} animated model instances to '{}' with key {}",
        ANIMATION_NAMES[next_index],
        next_index + 1
    );
}
