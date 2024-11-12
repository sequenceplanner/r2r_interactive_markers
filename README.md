# r2r_interactive_markers

A Rust implementation of http://wiki.ros.org/interactive_markers for use with r2r.

Interactive markers are similar to the "regular" markers, however they allow the user to interact with them by changing their position or rotation, clicking on them or selecting something from a context menu assigned to each marker.
They are represented by the visualization_msgs/InteractiveMarker message, which contains a context menu and several controls (visualization_msgs/InteractiveMarkerControl). The controls define the different visual parts of the interactive marker, can consist of several regular markers (visualization_msgs/Marker) and can each have a different function. 

## Run the examples:
```
cargo run --examples simple_marker
cargo run --examples cube
```