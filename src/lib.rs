use futures::{Stream, StreamExt};
use r2r::geometry_msgs::msg::Pose;
use r2r::std_msgs::msg::Header;
use r2r::visualization_msgs::msg::{
    InteractiveMarker, InteractiveMarkerFeedback, InteractiveMarkerPose, InteractiveMarkerUpdate,
};
use r2r::visualization_msgs::srv::GetInteractiveMarkers;
use r2r::{Publisher, QosProfile, ServiceRequest};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

type FeedbackCallbackBox = Arc<dyn Fn(InteractiveMarkerFeedback) + Send + Sync + 'static>;

const DEFAULT_FEEDBACK_CB: u8 = 255;

#[derive(Clone)]
enum UpdateType {
    FullUpdate,
    PoseUpdate,
    Erase,
}

// Struct to hold the information about a marker
#[derive(Clone)]
struct MarkerContext {
    pub last_feedback: SystemTime,
    pub last_client_id: String,
    pub default_feedback_cb: Option<FeedbackCallbackBox>,
    pub feedback_cbs: HashMap<u8, FeedbackCallbackBox>,
    pub int_marker: InteractiveMarker,
}

// Struct to hold the information about an update
#[derive(Clone)]
struct UpdateContext {
    pub update_type: UpdateType,
    pub int_marker: InteractiveMarker,
    pub default_feedback_cb: Option<FeedbackCallbackBox>,
    pub feedback_cbs: HashMap<u8, FeedbackCallbackBox>,
}

#[derive(Clone)]
pub struct InteractiveMarkerServer {
    pub topic_namespace: String,
    marker_contexts: Arc<Mutex<HashMap<String, MarkerContext>>>,
    pending_updates: Arc<Mutex<HashMap<String, UpdateContext>>>,
    pub sequence_number: Arc<AtomicU64>,
    pub update_pub: Publisher<InteractiveMarkerUpdate>,
}

impl InteractiveMarkerServer {
    pub fn new(
        topic_namespace: &str,
        node: Arc<Mutex<r2r::Node>>,
    ) -> Self {
        let update_topic = format!("{}/update", topic_namespace);
        let feedback_topic = format!("{}/feedback", topic_namespace);
        let service_name = format!("{}/get_interactive_markers", topic_namespace);

        let mut update_pub_qos = QosProfile::default();
        let mut feedback_sub_qos = QosProfile::default();

        update_pub_qos.depth = 100;
        feedback_sub_qos.depth = 1;

        let update_pub = node
            .lock()
            .unwrap()
            .create_publisher::<InteractiveMarkerUpdate>(&update_topic, update_pub_qos)
            .expect("Failed to create publisher");

        let marker_contexts = Arc::new(Mutex::new(HashMap::new()));
        let pending_updates = Arc::new(Mutex::new(HashMap::new()));
        let sequence_number = Arc::new(AtomicU64::new(0));

        let feedback_sub = node
            .lock()
            .unwrap()
            .subscribe::<InteractiveMarkerFeedback>(&feedback_topic, feedback_sub_qos)
            .unwrap();

        let marker_contexts_clone = Arc::clone(&marker_contexts);
        let pending_updates_clone = Arc::clone(&pending_updates);
        let sequence_number_clone = Arc::clone(&sequence_number);

        tokio::task::spawn(async move {
            match Self::feedback_subscriber_callback(
                feedback_sub,
                marker_contexts_clone,
                pending_updates_clone,
                sequence_number_clone,
            )
            .await
            {
                Ok(()) => (),
                Err(e) => r2r::log_error!("asdf", "Feedback subscriber failed with: '{}'.", e),
            }
        });

        let marker_contexts_clone = Arc::clone(&marker_contexts);
        let sequence_number_clone = Arc::clone(&sequence_number);

        let get_interactive_markers_service = node
            .lock()
            .unwrap()
            .create_service::<GetInteractiveMarkers::Service>(&service_name, QosProfile::default()).unwrap();

        tokio::task::spawn(async move {
            let result = Self::get_interactive_markers_server(
                get_interactive_markers_service,
                marker_contexts_clone,
                sequence_number_clone,
            )
            .await;
            match result {
                Ok(()) => r2r::log_info!("node", "Asdf succeeded."),
                Err(e) => r2r::log_error!("node", "Asdf service call failed with: {}.", e),
            };
        });


        Self {
            topic_namespace: topic_namespace.to_string(),
            marker_contexts,
            pending_updates,
            sequence_number,
            update_pub,
        }
    }

    async fn get_interactive_markers_server(
        mut service: impl Stream<Item = ServiceRequest<GetInteractiveMarkers::Service>> + Unpin,
        marker_contexts: Arc<Mutex<HashMap<String, MarkerContext>>>,
        sequence_number: Arc<AtomicU64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            match service.next().await {
                Some(request) => {
                    let response = GetInteractiveMarkers::Response {
                        sequence_number: sequence_number.load(Ordering::SeqCst),
                        markers: marker_contexts.lock().unwrap().values().map(|ctx| ctx.int_marker.clone()).collect()
                    };
                    request
                        .respond(response)
                        .expect("Could not send service response.");
                }
                None => ()
            }
        }
    }

    async fn feedback_subscriber_callback(
        mut subscriber: impl Stream<Item = InteractiveMarkerFeedback> + Unpin,
        marker_contexts: Arc<Mutex<HashMap<String, MarkerContext>>>,
        pending_updates: Arc<Mutex<HashMap<String, UpdateContext>>>,
        sequence_number: Arc<AtomicU64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        while let Some(feedback) = subscriber.next().await {
            Self::process_feedback(
                &marker_contexts,
                &pending_updates,
                &sequence_number,
                feedback,
            );
        }
        Ok(())
    }

    pub fn insert(&self, marker: InteractiveMarker) {
        let mut pending_updates = self.pending_updates.lock().unwrap();
        let name = marker.name.clone();

        let update_context = pending_updates
            .entry(name.clone())
            .or_insert_with(|| UpdateContext {
                update_type: UpdateType::FullUpdate,
                int_marker: marker.clone(),
                default_feedback_cb: None,
                feedback_cbs: HashMap::new(),
            });

        update_context.update_type = UpdateType::FullUpdate;
        update_context.int_marker = marker;

        println!("Marker inserted with name '{}'", name);
    }

    pub fn insert_with_callback(
        &self,
        marker: &InteractiveMarker,
        feedback_cb: Option<FeedbackCallbackBox>,
        feedback_type: u8,
    ) {
        self.insert(marker.clone());
        self.set_callback(&marker.name, feedback_cb, feedback_type);
    }

    pub fn set_callback(
        &self,
        name: &str,
        feedback_cb: Option<FeedbackCallbackBox>,
        feedback_type: u8,
    ) -> bool {
        let mut marker_contexts = self.marker_contexts.lock().unwrap();
        let mut pending_updates = self.pending_updates.lock().unwrap();

        let marker_exists =
            marker_contexts.contains_key(name) || pending_updates.contains_key(name);

        if !marker_exists {
            return false;
        }

        if let Some(marker_context) = marker_contexts.get_mut(name) {
            if feedback_type == DEFAULT_FEEDBACK_CB {
                marker_context.default_feedback_cb = feedback_cb.clone();
            } else {
                if let Some(callback) = feedback_cb.clone() {
                    marker_context.feedback_cbs.insert(feedback_type, callback);
                } else {
                    marker_context.feedback_cbs.remove(&feedback_type);
                }
            }
        }

        if let Some(update_context) = pending_updates.get_mut(name) {
            if feedback_type == DEFAULT_FEEDBACK_CB {
                update_context.default_feedback_cb = feedback_cb;
            } else {
                if let Some(callback) = feedback_cb {
                    update_context.feedback_cbs.insert(feedback_type, callback);
                } else {
                    update_context.feedback_cbs.remove(&feedback_type);
                }
            }
        }

        true
    }

    pub fn set_pose(&self, name: &str, pose: Pose, header: Option<Header>) -> bool {
        let marker_contexts = self.marker_contexts.lock().unwrap();
        let mut pending_updates = self.pending_updates.lock().unwrap();

        if !marker_contexts.contains_key(name) && !pending_updates.contains_key(name) {
            return false;
        }

        // Get the new_header before obtaining a mutable reference to pending_updates
        let new_header = if let Some(marker_context) = marker_contexts.get(name) {
            header.unwrap_or_else(|| marker_context.int_marker.header.clone())
        } else if let Some(existing_update) = pending_updates.get(name) {
            header.unwrap_or_else(|| existing_update.int_marker.header.clone())
        } else {
            // This should not happen due to the earlier check
            return false;
        };

        // Now obtain a mutable reference to the update context
        let update_context =
            pending_updates
                .entry(name.to_string())
                .or_insert_with(|| UpdateContext {
                    update_type: UpdateType::PoseUpdate,
                    int_marker: InteractiveMarker::default(),
                    default_feedback_cb: None,
                    feedback_cbs: HashMap::new(),
                });

        update_context.int_marker.pose = pose;
        update_context.int_marker.header = new_header;
        update_context.update_type = UpdateType::PoseUpdate;
        true
    }

    pub fn erase(&self, name: &str) -> bool {
        let marker_contexts = self.marker_contexts.lock().unwrap();
        let mut pending_updates = self.pending_updates.lock().unwrap();

        if !marker_contexts.contains_key(name) && !pending_updates.contains_key(name) {
            return false;
        }

        pending_updates.insert(
            name.to_string(),
            UpdateContext {
                update_type: UpdateType::Erase,
                int_marker: InteractiveMarker::default(),
                default_feedback_cb: None,
                feedback_cbs: HashMap::new(),
            },
        );
        true
    }

    pub fn clear(&self) {
        let mut pending_updates = self.pending_updates.lock().unwrap();
        pending_updates.clear();

        let marker_contexts = self.marker_contexts.lock().unwrap();
        for name in marker_contexts.keys() {
            pending_updates.insert(
                name.clone(),
                UpdateContext {
                    update_type: UpdateType::Erase,
                    int_marker: InteractiveMarker::default(),
                    default_feedback_cb: None,
                    feedback_cbs: HashMap::new(),
                },
            );
        }
    }

    pub fn empty(&self) -> bool {
        self.marker_contexts.lock().unwrap().is_empty()
    }

    pub fn size(&self) -> usize {
        self.marker_contexts.lock().unwrap().len()
    }

    pub fn apply_changes(&self) {
        let mut marker_contexts = self.marker_contexts.lock().unwrap();
        let mut pending_updates = self.pending_updates.lock().unwrap();
        let sequence_number = self.sequence_number.clone();

        if pending_updates.is_empty() {
            println!("No changes to apply");
            return;
        }

        let mut update = InteractiveMarkerUpdate::default();
        update.type_ = InteractiveMarkerUpdate::UPDATE as u8;
        update.markers = Vec::new();
        update.poses = Vec::new();
        update.erases = Vec::new();

        for (name, update_context) in pending_updates.iter() {
            match update_context.update_type {
                UpdateType::FullUpdate => {
                    let marker_context =
                        marker_contexts
                            .entry(name.clone())
                            .or_insert_with(|| MarkerContext {
                                last_feedback: SystemTime::now(),
                                last_client_id: "".to_string(),
                                default_feedback_cb: update_context.default_feedback_cb.clone(),
                                feedback_cbs: update_context.feedback_cbs.clone(),
                                int_marker: update_context.int_marker.clone(),
                            });
                    marker_context.int_marker = update_context.int_marker.clone();
                    marker_context.default_feedback_cb = update_context.default_feedback_cb.clone();
                    marker_context.feedback_cbs = update_context.feedback_cbs.clone();
                    update.markers.push(marker_context.int_marker.clone());
                }
                UpdateType::PoseUpdate => {
                    if let Some(marker_context) = marker_contexts.get_mut(name) {
                        marker_context.int_marker.pose = update_context.int_marker.pose.clone();
                        marker_context.int_marker.header = update_context.int_marker.header.clone();

                        let pose_update = InteractiveMarkerPose {
                            header: marker_context.int_marker.header.clone(),
                            pose: marker_context.int_marker.pose.clone(),
                            name: marker_context.int_marker.name.clone(),
                        };
                        update.poses.push(pose_update);
                    } else {
                        println!("Pending pose update for non-existing marker '{}'.", name);
                    }
                }
                UpdateType::Erase => {
                    marker_contexts.remove(name);
                    update.erases.push(name.clone());
                }
            }
        }

        let seq_num = sequence_number.fetch_add(1, Ordering::SeqCst) + 1;
        update.seq_num = seq_num;
        self.update_pub
            .publish(&update)
            .expect("Failed to publish update");
        pending_updates.clear();
    }

    fn process_feedback(
        marker_contexts: &Arc<Mutex<HashMap<String, MarkerContext>>>,
        pending_updates: &Arc<Mutex<HashMap<String, UpdateContext>>>,
        _sequence_number: &Arc<AtomicU64>,
        feedback: InteractiveMarkerFeedback,
    ) {
        let mut marker_contexts = marker_contexts.lock().unwrap();
        let name = feedback.marker_name.clone();

        if let Some(marker_context) = marker_contexts.get_mut(&name) {
            marker_context.last_feedback = SystemTime::now();
            marker_context.last_client_id = feedback.client_id.clone();

            if feedback.event_type == InteractiveMarkerFeedback::POSE_UPDATE as u8 {
                let mut pending_updates = pending_updates.lock().unwrap();
                let update_context =
                    pending_updates
                        .entry(name.clone())
                        .or_insert_with(|| UpdateContext {
                            update_type: UpdateType::PoseUpdate,
                            int_marker: InteractiveMarker::default(),
                            default_feedback_cb: None,
                            feedback_cbs: HashMap::new(),
                        });

                update_context.int_marker.pose = feedback.pose.clone();
                update_context.int_marker.header = feedback.header.clone();
                update_context.update_type = UpdateType::PoseUpdate;
            }

            let event_type = feedback.event_type;
            if let Some(callback) = marker_context.feedback_cbs.get(&event_type) {
                callback(feedback.clone());
            } else if let Some(callback) = &marker_context.default_feedback_cb {
                callback(feedback.clone());
            }
        } else {
            // This should also not happen
            println!("Received feedback for unknown marker '{}', ignoring.", name);
        }
    }

    pub fn get(&self, name: &str) -> Option<InteractiveMarker> {
        let marker_contexts = self.marker_contexts.lock().unwrap();
        let pending_updates = self.pending_updates.lock().unwrap();

        if let Some(update_context) = pending_updates.get(name) {
            match update_context.update_type {
                UpdateType::Erase => None,
                UpdateType::FullUpdate => Some(update_context.int_marker.clone()),
                UpdateType::PoseUpdate => {
                    if let Some(marker_context) = marker_contexts.get(name) {
                        let mut marker = marker_context.int_marker.clone();
                        marker.pose = update_context.int_marker.pose.clone();
                        Some(marker)
                    } else {
                        None
                    }
                }
            }
        } else if let Some(marker_context) = marker_contexts.get(name) {
            Some(marker_context.int_marker.clone())
        } else {
            None
        }
    }
}
