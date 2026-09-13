use std::{collections::HashMap, time::Duration};

use avian3d::{math::Vector, prelude::*};
use bevy::{
    app::AnimationSystems,
    asset::AssetPlugin,
    camera::visibility::NoFrustumCulling,
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    gltf::GltfAssetLabel,
    prelude::*,
    transform::TransformSystems,
    world_serialization::WorldInstanceReady,
};

const MODEL_PATH: &str = "standingholdingjaw.glb";
const MODEL_CENTER: Vec3 = Vec3::new(0.0, 2.3, 0.18);
const ANIMATION_NAMES: [&str; 2] = ["walking", "whypausegame"];

// Angle compliance of the ragdoll joints (inverse stiffness, N*m/rad). This is
// the "elasticity" knob: smaller = stiffer / more rigid body that holds its
// pose; larger = softer and floppier. XPBD compliance is stable at any value.
const JOINT_ANGLE_COMPLIANCE: f32 = 0.3;

// Thin leaf bodies (low twist inertia) jitter/spin under an angle constraint, so
// they get free (point-only) joints and just dangle. Forearms (thin capsules at
// the end of the arm) and feet both need this; the elbow and ankle end up free
// while shoulders, knees, and spine keep their angle-constraint "muscle tone".
const FREE_LEAF_BONES: [&str; 4] = ["forearm_stretch.l", "forearm_stretch.r", "foot.l", "foot.r"];

// Mouse grab/throw: how hard the grabbed part chases the cursor (1/s), and the
// max speed it can be flung at (m/s). Higher stiffness = snappier, harder throws.
const GRAB_STIFFNESS: f32 = 30.0;
const GRAB_MAX_SPEED: f32 = 50.0;

#[derive(Resource)]
struct Animations {
    graph_handle: Handle<AnimationGraph>,
    node_indices: Vec<AnimationNodeIndex>,
    current_index: usize,
}

#[derive(Resource, Default)]
struct RagdollMode {
    dynamic: bool,
}

/// Marker for the joint entities so they can be despawned when ragdoll is toggled off.
#[derive(Component)]
struct RagdollJoint;

#[derive(Component)]
struct AnimatedModelRoot;

#[derive(Component)]
struct RagdollPart {
    bone_entity: Entity,
}

/// Active mouse grab: which ragdoll body is held, the depth it was grabbed at,
/// and where on the body (local space) the grab point is.
#[derive(Resource)]
struct Grab {
    body: Entity,
    grab_distance: f32,
    local_anchor: Vec3,
}

/// Collision layers: ragdoll parts collide with the ground but never with each
/// other. Their colliders overlap at the bind pose, so self-collision would
/// blast them apart on the first dynamic step (the "fly away / squeeze" look).
#[derive(PhysicsLayer, Clone, Copy, Debug, Default)]
enum GameLayer {
    #[default]
    Default,
    Ground,
    Ragdoll,
}

#[derive(Clone, Copy)]
enum RagdollShape {
    Sphere { radius: f32 },
    Capsule { radius: f32, length: f32 },
    Cuboid { size: Vec3 },
}

#[derive(Clone, Copy)]
struct RagdollPartSpec {
    bone_name: &'static str,
    shape: RagdollShape,
}

const RAGDOLL_PARTS: [RagdollPartSpec; 14] = [
    RagdollPartSpec {
        bone_name: "root.x",
        shape: RagdollShape::Cuboid {
            size: Vec3::new(0.35, 0.25, 0.28),
        },
    },
    RagdollPartSpec {
        bone_name: "spine_01.x",
        shape: RagdollShape::Capsule {
            radius: 0.16,
            length: 0.45,
        },
    },
    RagdollPartSpec {
        bone_name: "spine_02.x",
        shape: RagdollShape::Capsule {
            radius: 0.15,
            length: 0.42,
        },
    },
    RagdollPartSpec {
        bone_name: "head.x",
        shape: RagdollShape::Sphere { radius: 0.2 },
    },
    // Arm = upper arm (shoulder) + forearm; the hand is no longer its own body.
    RagdollPartSpec {
        bone_name: "shoulder.l",
        shape: RagdollShape::Capsule {
            radius: 0.08,
            length: 0.45,
        },
    },
    RagdollPartSpec {
        bone_name: "forearm_stretch.l",
        shape: RagdollShape::Capsule {
            radius: 0.07,
            length: 0.40,
        },
    },
    RagdollPartSpec {
        bone_name: "shoulder.r",
        shape: RagdollShape::Capsule {
            radius: 0.08,
            length: 0.45,
        },
    },
    RagdollPartSpec {
        bone_name: "forearm_stretch.r",
        shape: RagdollShape::Capsule {
            radius: 0.07,
            length: 0.40,
        },
    },
    // Leg = thigh + foreleg (shin) + foot.
    RagdollPartSpec {
        bone_name: "thigh_stretch.l",
        shape: RagdollShape::Capsule {
            radius: 0.1,
            length: 0.55,
        },
    },
    RagdollPartSpec {
        bone_name: "leg_stretch.l",
        shape: RagdollShape::Capsule {
            radius: 0.09,
            length: 0.50,
        },
    },
    RagdollPartSpec {
        bone_name: "foot.l",
        shape: RagdollShape::Cuboid {
            size: Vec3::new(0.15, 0.12, 0.28),
        },
    },
    RagdollPartSpec {
        bone_name: "thigh_stretch.r",
        shape: RagdollShape::Capsule {
            radius: 0.1,
            length: 0.55,
        },
    },
    RagdollPartSpec {
        bone_name: "leg_stretch.r",
        shape: RagdollShape::Capsule {
            radius: 0.09,
            length: 0.50,
        },
    },
    RagdollPartSpec {
        bone_name: "foot.r",
        shape: RagdollShape::Cuboid {
            size: Vec3::new(0.15, 0.12, 0.28),
        },
    },
];

const RAGDOLL_JOINTS: [(&str, &str); 13] = [
    ("root.x", "spine_01.x"),
    ("spine_01.x", "spine_02.x"),
    ("spine_02.x", "head.x"),
    ("spine_02.x", "shoulder.l"),
    ("shoulder.l", "forearm_stretch.l"),
    ("spine_02.x", "shoulder.r"),
    ("shoulder.r", "forearm_stretch.r"),
    ("root.x", "thigh_stretch.l"),
    ("thigh_stretch.l", "leg_stretch.l"),
    ("leg_stretch.l", "foot.l"),
    ("root.x", "thigh_stretch.r"),
    ("thigh_stretch.r", "leg_stretch.r"),
    ("leg_stretch.r", "foot.r"),
];

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
                        title: "GLB model viewer + Avian ragdoll".into(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins((FreeCameraPlugin, PhysicsPlugins::default(), PhysicsDebugPlugin))
        .insert_resource(ClearColor(Color::srgb(0.12, 0.13, 0.14)))
        .insert_resource(GlobalAmbientLight {
            brightness: 1200.0,
            ..default()
        })
        .insert_resource(SubstepCount(60))
        .insert_resource(Gravity(Vector::NEG_Y * 9.81))
        .init_resource::<RagdollMode>()
        .add_systems(Startup, setup)
        .add_systems(Update, (switch_animation, toggle_ragdoll, grab_ragdoll))
        .add_systems(
            PostUpdate,
            // Must run AFTER the animation systems: a paused `AnimationPlayer`
            // still writes its frozen pose to the bone `Transform`s every frame
            // inside `animate_targets`. Without this ordering the animation
            // non-deterministically clobbers the ragdoll's bone writes, so the
            // skinned mesh stays frozen while the (separate) physics colliders
            // visibly move. Still before `Propagate` so skinning sees the result.
            drive_bones_from_ragdoll
                .after(AnimationSystems)
                .before(TransformSystems::Propagate),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut animation_graphs: ResMut<Assets<AnimationGraph>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (animation_graph, node_indices) = AnimationGraph::from_clips([
        asset_server.load(GltfAssetLabel::Animation(0).from_asset(MODEL_PATH)),
        asset_server.load(GltfAssetLabel::Animation(1).from_asset(MODEL_PATH)),
    ]);

    commands.insert_resource(Animations {
        graph_handle: animation_graphs.add(animation_graph),
        node_indices,
        current_index: 0,
    });

    commands
        .spawn((
            AnimatedModelRoot,
            // Bevy 0.19: `SceneRoot` (Handle<Scene>) became `WorldAssetRoot`
            // (Handle<WorldAsset>) when the old scene crate became
            // `bevy_world_serialization`. glTF scenes load as WorldAssets now.
            WorldAssetRoot(asset_server.load(GltfAssetLabel::Scene(0).from_asset(MODEL_PATH))),
        ))
        .observe(play_animation_and_build_ragdoll_when_ready);

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.18, 0.19, 0.2),
            perceptual_roughness: 0.85,
            ..default()
        })),
        RigidBody::Static,
        Collider::cuboid(12.0, 0.1, 12.0),
        CollisionLayers::new(GameLayer::Ground, LayerMask::ALL),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 2.5, 7.0).looking_at(MODEL_CENTER, Vec3::Y),
        FreeCamera {
            mouse_key_cursor_grab: MouseButton::Right,
            walk_speed: 2.5,
            run_speed: 8.0,
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
            intensity: 600_000.0,
            range: 12.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(0.0, 4.0, 4.0),
    ));

    info!(
        "Controls: 1/2 switch animation, Enter cycles animation, R toggles ragdoll physics, \
         Left-drag to grab & throw the ragdoll"
    );
}

fn play_animation_and_build_ragdoll_when_ready(
    scene_ready: On<WorldInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    animations: Res<Animations>,
    mut players: Query<&mut AnimationPlayer>,
    mesh_entities: Query<(), With<Mesh3d>>,
) {
    // The skinned mesh's `Aabb` is computed once from the bind pose and never
    // updated for skinning deformation, so a ragdoll that flops far from the
    // rest pose would be frustum-culled and vanish. Opt these meshes out.
    for child in children.iter_descendants(scene_ready.entity) {
        if mesh_entities.contains(child) {
            commands.entity(child).insert(NoFrustumCulling);
        }
    }

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

        info!("Looping glTF animation '{}'", ANIMATION_NAMES[animations.current_index]);
    }
}

/// Spawns the dynamic ragdoll bodies + joints from the CURRENT animated bone
/// poses. Called when ragdoll is toggled on, so the bones always have valid,
/// up-to-date world transforms (no scene-spawn timing issues) and the bodies
/// start exactly on the visible pose with correct joint anchors.
fn spawn_ragdoll(
    commands: &mut Commands,
    named_bones: &Query<(Entity, &Name, &GlobalTransform)>,
) -> (usize, usize) {
    let mut bone_transforms = HashMap::<&str, (Entity, Transform)>::new();
    for (entity, name, global_transform) in named_bones {
        bone_transforms.insert(name.as_str(), (entity, global_transform.compute_transform()));
    }

    let mut ragdoll_bodies = HashMap::<&str, (Entity, Vec3)>::new();
    for spec in RAGDOLL_PARTS {
        let Some((bone_entity, transform)) = bone_transforms.get(spec.bone_name) else {
            warn!("Ragdoll bone '{}' was not found in the loaded scene", spec.bone_name);
            continue;
        };

        let collider = collider_for(spec.shape);
        let entity = commands
            .spawn((
                RagdollPart {
                    bone_entity: *bone_entity,
                },
                RigidBody::Dynamic,
                collider,
                ColliderDensity(1.0),
                LinearDamping(1.0),
                AngularDamping(2.0),
                MaxAngularSpeed(12.0),
                CollisionLayers::new(GameLayer::Ragdoll, GameLayer::Ground),
                DebugRender::default().with_collider_color(Color::srgb(0.1, 0.8, 1.0)),
                *transform,
            ))
            .id();

        ragdoll_bodies.insert(spec.bone_name, (entity, transform.translation));
    }

    let mut joint_count = 0;
    for (parent_name, child_name) in RAGDOLL_JOINTS {
        let (Some((parent, _)), Some((child, _))) =
            (ragdoll_bodies.get(parent_name), ragdoll_bodies.get(child_name))
        else {
            continue;
        };
        let (Some((_, parent_tf)), Some((_, child_tf))) =
            (bone_transforms.get(parent_name), bone_transforms.get(child_name))
        else {
            continue;
        };

        // Joint pivot = the child bone's origin in world space, converted into
        // each body's local frame so the anchors are correct and deterministic.
        let pivot = child_tf.translation;
        let local_anchor1 = parent_tf.compute_affine().inverse().transform_point3(pivot);
        let local_anchor2 = child_tf.compute_affine().inverse().transform_point3(pivot);

        if FREE_LEAF_BONES.contains(&child_name) {
            // Hands/feet: free spherical joint (point only) so they dangle
            // without the angle-constraint jitter that plagues tiny bodies.
            commands.spawn((
                RagdollJoint,
                SphericalJoint::new(*parent, *child)
                    .with_local_anchor1(local_anchor1)
                    .with_local_anchor2(local_anchor2)
                    .with_point_compliance(0.00001),
            ));
        } else {
            // Core body: `FixedJoint` with a near-rigid point constraint (limbs
            // stay attached) plus a *compliant* angle constraint, so the body
            // holds its pose with springy "muscle tone" and deforms elastically
            // instead of flopping limp. XPBD compliance is stable for any
            // stiffness (unlike an explicit torque spring). `local_basis2` makes
            // the CURRENT relative orientation the rest pose, so the joint
            // doesn't snap bent joints straight.
            let rest_basis2 = child_tf.rotation.inverse() * parent_tf.rotation;
            commands.spawn((
                RagdollJoint,
                FixedJoint::new(*parent, *child)
                    .with_local_anchor1(local_anchor1)
                    .with_local_anchor2(local_anchor2)
                    .with_local_basis2(rest_basis2)
                    .with_point_compliance(0.00001)
                    .with_angle_compliance(JOINT_ANGLE_COMPLIANCE),
            ));
        }
        joint_count += 1;
    }

    (ragdoll_bodies.len(), joint_count)
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

    animations.current_index = next_index;
    let next_animation = animations.node_indices[next_index];

    for (mut player, mut transitions) in &mut players {
        transitions
            .play(&mut player, next_animation, Duration::from_millis(250))
            .repeat();
    }

    info!("Switched animation to '{}'", ANIMATION_NAMES[next_index]);
}

fn toggle_ragdoll(
    mut commands: Commands,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<RagdollMode>,
    ragdoll_parts: Query<Entity, With<RagdollPart>>,
    ragdoll_joints: Query<Entity, With<RagdollJoint>>,
    named_bones: Query<(Entity, &Name, &GlobalTransform)>,
    mut animation_players: Query<&mut AnimationPlayer>,
) {
    if !keyboard_input.just_pressed(KeyCode::KeyR) {
        return;
    }

    mode.dynamic = !mode.dynamic;

    if mode.dynamic {
        // Build the ragdoll from the current pose, then freeze the animation so
        // physics owns the bones (`drive_bones_from_ragdoll` does the driving).
        let (bodies, joints) = spawn_ragdoll(&mut commands, &named_bones);
        for mut player in &mut animation_players {
            player.pause_all();
        }
        info!("Ragdoll ON: {bodies} bodies, {joints} joints (physics drives the mesh)");
    } else {
        // Tear the ragdoll down and hand the bones back to the animation player.
        for entity in &ragdoll_parts {
            commands.entity(entity).despawn();
        }
        for entity in &ragdoll_joints {
            commands.entity(entity).despawn();
        }
        for mut player in &mut animation_players {
            player.resume_all();
        }
        info!("Ragdoll OFF: animation resumed");
    }
}


fn drive_bones_from_ragdoll(
    mode: Res<RagdollMode>,
    ragdoll_parts: Query<(&RagdollPart, &Transform), With<RagdollPart>>,
    mut bones: Query<(Entity, &mut Transform, Option<&ChildOf>), Without<RagdollPart>>,
    bone_globals: Query<&GlobalTransform, Without<RagdollPart>>,
) {
    if !mode.dynamic {
        return;
    }

    // World-space pose each driven bone should land at, taken from its physics body.
    let ragdoll_targets = ragdoll_parts
        .iter()
        .map(|(part, transform)| (part.bone_entity, GlobalTransform::from(*transform)))
        .collect::<HashMap<_, _>>();

    // Snapshot every bone's current local transform + parent. Several driven
    // bones (head/hands/feet) have an *undriven* intermediate bone as their real
    // skeleton parent (e.g. hand.l -> forearm_stretch.l). The propagated
    // GlobalTransform of those intermediates is still one frame stale here, so we
    // rebuild parent globals ourselves to avoid a lagging, stretched look.
    let mut local_map = HashMap::<Entity, Transform>::new();
    let mut parent_map = HashMap::<Entity, Option<Entity>>::new();
    for (entity, transform, parent) in &bones {
        local_map.insert(entity, *transform);
        parent_map.insert(entity, parent.map(|child_of| child_of.0));
    }

    // Express each physics world target relative to its parent's *current-frame*
    // global, so that after propagation (parent_global * local) the bone lands
    // exactly on the target with no frame lag.
    let mut cache = HashMap::<Entity, GlobalTransform>::new();
    let mut new_locals = HashMap::<Entity, Transform>::new();
    for (&bone_entity, target_global) in &ragdoll_targets {
        let new_local = match parent_map.get(&bone_entity).copied().flatten() {
            Some(parent) => {
                let parent_global = fresh_bone_global(
                    parent,
                    &ragdoll_targets,
                    &local_map,
                    &parent_map,
                    &bone_globals,
                    &mut cache,
                );
                target_global.reparented_to(&parent_global)
            }
            None => target_global.compute_transform(),
        };
        new_locals.insert(bone_entity, new_local);
    }

    for (entity, mut transform, _) in &mut bones {
        if let Some(new_local) = new_locals.get(&entity) {
            *transform = *new_local;
        }   
    }
}

/// World-space transform a bone will have *this* frame: a physics-driven bone is
/// pinned to its body's pose; an undriven bone rides its parent at its current
/// local offset (recursively), so reparenting a child onto it adds no frame lag.
fn fresh_bone_global(
    entity: Entity,
    ragdoll_targets: &HashMap<Entity, GlobalTransform>,
    local_map: &HashMap<Entity, Transform>,
    parent_map: &HashMap<Entity, Option<Entity>>,
    fallback_globals: &Query<&GlobalTransform, Without<RagdollPart>>,
    cache: &mut HashMap<Entity, GlobalTransform>,
) -> GlobalTransform {
    if let Some(target) = ragdoll_targets.get(&entity) {
        return *target;
    }
    if let Some(cached) = cache.get(&entity) {
        return *cached;
    }

    let result = match (parent_map.get(&entity).copied().flatten(), local_map.get(&entity)) {
        (Some(parent), Some(local)) => {
            let parent_global = fresh_bone_global(
                parent,
                ragdoll_targets,
                local_map,
                parent_map,
                fallback_globals,
                cache,
            );
            parent_global * *local
        }
        // Untracked parent (e.g. the static armature root): its last propagated
        // global is accurate since it isn't being driven this frame.
        _ => fallback_globals.get(entity).copied().unwrap_or_default(),
    };

    cache.insert(entity, result);
    result
}

// TEMP DIAGNOSTIC: after the model has settled into the animation, log where
// each ragdoll part is vs. where its target bone actually is, to find why the
// parts cluster at the center. Remove once fixed.
// Left-mouse grab & throw. Raycast from the camera through the cursor to a
// ragdoll part; while held, drive that part's velocity so its grab point chases
// the cursor (kept at the depth it was grabbed at). Releasing leaves the body
// with whatever velocity it had -> flick the cursor to throw it.
fn grab_ragdoll(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    spatial_query: SpatialQuery,
    bodies: Query<(&Position, &Rotation), With<RagdollPart>>,
    mut velocities: Query<&mut LinearVelocity, With<RagdollPart>>,
    grab: Option<Res<Grab>>,
    mut commands: Commands,
) {
    if mouse.just_released(MouseButton::Left) {
        if grab.is_some() {
            commands.remove_resource::<Grab>();
        }
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok((camera, camera_transform)) = cameras.single() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else {
        return;
    };

    if let Some(grab) = grab {
        // Holding: pull the grabbed point toward the cursor at the grab depth.
        let Ok((position, rotation)) = bodies.get(grab.body) else {
            commands.remove_resource::<Grab>();
            return;
        };
        let target = ray.origin + ray.direction * grab.grab_distance;
        let grab_point = position.0 + rotation.0 * grab.local_anchor;
        if let Ok(mut velocity) = velocities.get_mut(grab.body) {
            velocity.0 = ((target - grab_point) * GRAB_STIFFNESS).clamp_length_max(GRAB_MAX_SPEED);
        }
    } else if mouse.just_pressed(MouseButton::Left) {
        // Start a grab if the cursor ray hits a ragdoll part.
        let filter = SpatialQueryFilter::from_mask(GameLayer::Ragdoll);
        if let Some(hit) = spatial_query.cast_ray(ray.origin, ray.direction, 1000.0, true, &filter) {
            if let Ok((position, rotation)) = bodies.get(hit.entity) {
                let hit_point = ray.origin + ray.direction * hit.distance;
                commands.insert_resource(Grab {
                    body: hit.entity,
                    grab_distance: hit.distance,
                    local_anchor: rotation.0.inverse() * (hit_point - position.0),
                });
            }
        }
    }
}

fn collider_for(shape: RagdollShape) -> Collider {
    match shape {
        RagdollShape::Sphere { radius } => Collider::sphere(radius),
        RagdollShape::Capsule { radius, length } => Collider::capsule(radius, length),
        RagdollShape::Cuboid { size } => Collider::cuboid(size.x, size.y, size.z),
    }
}
