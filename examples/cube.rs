use r2r_interactive_markers::InteractiveMarkerServer;
use r2r::geometry_msgs::msg::Pose;
use r2r::std_msgs::msg::Header;
use r2r::tf2_msgs::msg::TFMessage;
use r2r::visualization_msgs::msg::{
    InteractiveMarker, InteractiveMarkerControl, InteractiveMarkerFeedback, Marker,
};
use r2r::Context;
use r2r::QosProfile;
use std::sync::{Arc, Mutex};

pub static NODE_ID: &'static str = "cube";
const DEFAULT_FEEDBACK_CB: u8 = 255;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = Context::create()?;
    let node = r2r::Node::create(context, "cube", "")?;
    let node = Arc::new(Mutex::new(node));

    // We need to publish a frame where the marker can be placed
    let broadcasted_frames = Arc::new(Mutex::new(make_initial_tf()));
    let node_clone = node.clone();
    let static_pub_timer = node_clone
        .lock()
        .unwrap()
        .create_wall_timer(std::time::Duration::from_millis(100))?;
    let static_frame_broadcaster = node_clone.lock().unwrap().create_publisher::<TFMessage>(
        "tf_static",
        QosProfile::transient_local(QosProfile::default()),
    )?;
    let broadcasted_frames_clone = broadcasted_frames.clone();
    tokio::task::spawn(async move {
        match static_frame_broadcaster_callback(
            static_frame_broadcaster,
            static_pub_timer,
            &broadcasted_frames_clone,
        )
        .await
        {
            Ok(()) => (),
            Err(e) => r2r::log_error!(NODE_ID, "Static frame broadcaster failed with: '{}'.", e),
        };
    });

    let node_clone = node.clone();
    let server = Arc::new(InteractiveMarkerServer::new("cube", node_clone));

    let positions = Arc::new(Mutex::new(Vec::new()));

    make_cube(server.clone(), positions.clone());

    server.apply_changes();

    r2r::log_info!(NODE_ID, "Node started.");

    loop {
        {
            let positions_lock = positions.lock().unwrap();
            for (i, position) in positions_lock.iter().enumerate() {
                let mut pose = Pose::default();
                pose.position.x = position[0];
                pose.position.y = position[1];
                pose.position.z = position[2];

                server.set_pose(&i.to_string(), pose.clone(), None);
            }
        }
        server.apply_changes();

        node.lock()
            .unwrap()
            .spin_once(std::time::Duration::from_millis(100));
    }
}

fn make_cube(server: Arc<InteractiveMarkerServer>, positions: Arc<Mutex<Vec<[f64; 3]>>>) {
    let side_length = 10;
    let step = 1.0 / side_length as f64;
    let mut count = 0;

    for i in 0..side_length {
        let x = -0.5 + step * i as f64;
        for j in 0..side_length {
            let y = -0.5 + step * j as f64;
            for k in 0..side_length {
                let z = step * k as f64;

                let mut marker = InteractiveMarker::default();
                marker.header.frame_id = "base_link".to_string();
                marker.scale = step as f32;

                marker.pose.position.x = x;
                marker.pose.position.y = y;
                marker.pose.position.z = z;

                // Add position to positions vector
                positions.lock().unwrap().push([x, y, z]);

                marker.name = count.to_string();

                make_box_control(&mut marker);

                let positions_clone = positions.clone();

                // Create the feedback callback without capturing server_clone
                let feedback_cb = Arc::new(move |feedback: InteractiveMarkerFeedback| {
                    process_feedback(feedback, positions_clone.clone());
                });

                server.insert(marker);
                server.set_callback(&count.to_string(), Some(feedback_cb), DEFAULT_FEEDBACK_CB);

                count += 1;
            }
        }
    }
}

fn make_box_control(marker: &mut InteractiveMarker) {
    let mut control = InteractiveMarkerControl::default();
    control.always_visible = true;
    control.orientation_mode = InteractiveMarkerControl::VIEW_FACING as u8;
    control.interaction_mode = InteractiveMarkerControl::MOVE_PLANE as u8;
    control.independent_marker_orientation = true;

    let mut cube_marker = Marker::default();
    cube_marker.type_ = Marker::CUBE as i32;
    cube_marker.scale.x = marker.scale as f64;
    cube_marker.scale.y = marker.scale as f64;
    cube_marker.scale.z = marker.scale as f64;
    cube_marker.color.r = (0.65 + 0.7 * marker.pose.position.x) as f32;
    cube_marker.color.g = (0.65 + 0.7 * marker.pose.position.y) as f32;
    cube_marker.color.b = (0.65 + 0.7 * marker.pose.position.z) as f32;
    cube_marker.color.a = 1.0;

    control.markers.push(cube_marker);
    marker.controls.push(control);
}

fn process_feedback(feedback: InteractiveMarkerFeedback, positions: Arc<Mutex<Vec<[f64; 3]>>>) {
    if feedback.event_type == InteractiveMarkerFeedback::POSE_UPDATE as u8 {
        let x = feedback.pose.position.x;
        let y = feedback.pose.position.y;
        let z = feedback.pose.position.z;

        let index: usize = feedback.marker_name.parse().unwrap_or(0);

        let mut positions_lock = positions.lock().unwrap();

        if index >= positions_lock.len() {
            return;
        }

        let dx = x - positions_lock[index][0];
        let dy = y - positions_lock[index][1];
        let dz = z - positions_lock[index][2];

        // Move all markers in that direction
        for i in 0..positions_lock.len() {
            let (mx, my, mz) = (
                positions_lock[i][0],
                positions_lock[i][1],
                positions_lock[i][2],
            );
            let d = ((x - mx).powi(2) + (y - my).powi(2) + (z - mz).powi(2)).sqrt();
            let mut t = 1.0 / (d * 5.0 + 1.0) - 0.2;
            if t < 0.0 {
                t = 0.0;
            }

            positions_lock[i][0] += t * dx;
            positions_lock[i][1] += t * dy;
            positions_lock[i][2] += t * dz;

            if i == index {
                println!("{}", d);
                positions_lock[i][0] = x;
                positions_lock[i][1] = y;
                positions_lock[i][2] = z;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameData {
    pub parent_frame_id: String,
    pub child_frame_id: String,
    pub transform: r2r::geometry_msgs::msg::Transform,
    pub active: Option<bool>,
}

fn make_initial_tf() -> std::collections::HashMap<String, FrameData> {
    let mut test_setup = std::collections::HashMap::<String, FrameData>::new();

    test_setup.insert(
        "base_link".to_string(),
        FrameData {
            parent_frame_id: "world".to_string(),
            child_frame_id: "base_link".to_string(),
            transform: r2r::geometry_msgs::msg::Transform {
                translation: r2r::geometry_msgs::msg::Vector3 {
                    x: 0.0,
                    y: 0.0,
                    z: 1.0,
                },
                rotation: r2r::geometry_msgs::msg::Quaternion {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
            },
            active: Some(false),
        },
    );
    test_setup
}

async fn static_frame_broadcaster_callback(
    publisher: r2r::Publisher<TFMessage>,
    mut timer: r2r::Timer,
    frames: &Arc<Mutex<std::collections::HashMap<String, FrameData>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let mut clock = r2r::Clock::create(r2r::ClockType::RosTime).unwrap();
        let now = clock.get_now().unwrap();
        let time_stamp = r2r::Clock::to_builtin_time(&now);

        let transforms_local = frames.lock().unwrap().clone();
        let mut updated_transforms = vec![];

        transforms_local.iter().for_each(|(_, v)| match v.active {
            Some(false) => {
                updated_transforms.push(r2r::geometry_msgs::msg::TransformStamped {
                    header: Header {
                        stamp: time_stamp.clone(),
                        frame_id: v.parent_frame_id.clone(),
                    },
                    child_frame_id: v.child_frame_id.clone(),
                    transform: v.transform.clone(),
                });
            }
            Some(true) | None => (),
        });

        let msg = TFMessage {
            transforms: updated_transforms,
        };

        match publisher.publish(&msg) {
            Ok(()) => (),
            Err(e) => {
                r2r::log_error!(
                    NODE_ID,
                    "Static broadcaster failed to send a message with: '{}'",
                    e
                );
            }
        };
        timer.tick().await?;
    }
}
