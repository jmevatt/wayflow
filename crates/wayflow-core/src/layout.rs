// Virtual desktop layout and screen edge detection.
//
// The server may have multiple monitors arranged in a 2D virtual desktop.
// ServerLayout computes the external boundaries of that desktop and maps
// cursor positions to client screen coordinates when an edge is crossed.

use crate::config::Edge;
use wayflow_proto::ScreenInfo;

pub struct ServerLayout {
    monitors: Vec<ScreenInfo>,
}

impl ServerLayout {
    pub fn new(monitors: Vec<ScreenInfo>) -> Self {
        Self { monitors }
    }

    pub fn monitor_count(&self) -> usize {
        self.monitors.len()
    }

    /// Rightmost pixel x of the virtual desktop at cursor y.
    /// Returns None if y is not covered by any monitor.
    pub fn right_boundary_at(&self, y: i32) -> Option<i32> {
        self.monitors
            .iter()
            .filter(|m| y >= m.y && y < m.y + m.height as i32)
            .map(|m| m.x + m.width as i32 - 1)
            .max()
    }

    /// Leftmost pixel x of the virtual desktop at cursor y.
    pub fn left_boundary_at(&self, y: i32) -> Option<i32> {
        self.monitors
            .iter()
            .filter(|m| y >= m.y && y < m.y + m.height as i32)
            .map(|m| m.x)
            .min()
    }

    /// Bottommost pixel y of the virtual desktop at cursor x.
    pub fn bottom_boundary_at(&self, x: i32) -> Option<i32> {
        self.monitors
            .iter()
            .filter(|m| x >= m.x && x < m.x + m.width as i32)
            .map(|m| m.y + m.height as i32 - 1)
            .max()
    }

    /// Topmost pixel y of the virtual desktop at cursor x.
    pub fn top_boundary_at(&self, x: i32) -> Option<i32> {
        self.monitors
            .iter()
            .filter(|m| x >= m.x && x < m.x + m.width as i32)
            .map(|m| m.y)
            .min()
    }

    fn on_any_monitor(&self, cx: i32, cy: i32) -> bool {
        self.monitors.iter().any(|m| {
            cx >= m.x && cx < m.x + m.width as i32 &&
            cy >= m.y && cy < m.y + m.height as i32
        })
    }

    pub fn at_right_edge(&self, cx: i32, cy: i32) -> bool {
        self.on_any_monitor(cx, cy) &&
        self.right_boundary_at(cy).map(|b| cx >= b).unwrap_or(false)
    }

    pub fn at_left_edge(&self, cx: i32, cy: i32) -> bool {
        self.on_any_monitor(cx, cy) &&
        self.left_boundary_at(cy).map(|b| cx <= b).unwrap_or(false)
    }

    pub fn at_bottom_edge(&self, cx: i32, cy: i32) -> bool {
        self.on_any_monitor(cx, cy) &&
        self.bottom_boundary_at(cx).map(|b| cy >= b).unwrap_or(false)
    }

    pub fn at_top_edge(&self, cx: i32, cy: i32) -> bool {
        self.on_any_monitor(cx, cy) &&
        self.top_boundary_at(cx).map(|b| cy <= b).unwrap_or(false)
    }

    /// Returns which edge the cursor is at, if any. Right/Left take priority over Bottom/Top.
    pub fn crossed_edge(&self, cx: i32, cy: i32) -> Option<Edge> {
        if self.at_right_edge(cx, cy)  { return Some(Edge::Right); }
        if self.at_left_edge(cx, cy)   { return Some(Edge::Left); }
        if self.at_bottom_edge(cx, cy) { return Some(Edge::Bottom); }
        if self.at_top_edge(cx, cy)    { return Some(Edge::Top); }
        None
    }
}

/// Map a server cursor position to client screen coordinates when crossing an edge.
///
/// `offset` is the pixel shift along the perpendicular axis -- for Left/Right edges
/// it's vertical (positive = client top is below server top); for Top/Bottom edges
/// it's horizontal (positive = client left is right of server left).
pub fn map_to_client(
    server_x: i32,
    server_y: i32,
    client: &ScreenInfo,
    edge: Edge,
    offset: i32,
) -> (u16, u16) {
    let cw = client.width as i32;
    let ch = client.height as i32;
    let (cx, cy) = match edge {
        Edge::Right  => (0,        (server_y - offset).clamp(0, ch - 1)),
        Edge::Left   => (cw - 1,   (server_y - offset).clamp(0, ch - 1)),
        Edge::Bottom => ((server_x - offset).clamp(0, cw - 1), 0),
        Edge::Top    => ((server_x - offset).clamp(0, cw - 1), ch - 1),
    };
    (cx as u16, cy as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayflow_proto::ScreenInfo;

    fn mon(x: i32, y: i32, w: u16, h: u16) -> ScreenInfo {
        ScreenInfo { name: String::new(), x, y, width: w, height: h }
    }

    fn client(w: u16, h: u16) -> ScreenInfo {
        mon(0, 0, w, h)
    }

    fn single() -> ServerLayout {
        // 2560x1440 at origin
        ServerLayout::new(vec![mon(0, 0, 2560, 1440)])
    }

    fn dual_side_by_side() -> ServerLayout {
        // DP-1: 2560x1440 at (0,0), DP-2: 1920x1080 at (2560,0)
        ServerLayout::new(vec![mon(0, 0, 2560, 1440), mon(2560, 0, 1920, 1080)])
    }

    fn dual_stacked() -> ServerLayout {
        // Two 2560x1440 monitors stacked vertically
        ServerLayout::new(vec![mon(0, 0, 2560, 1440), mon(0, 1440, 2560, 1440)])
    }

    // ---- single monitor boundaries ----

    #[test]
    fn single_right_boundary() {
        assert_eq!(single().right_boundary_at(500), Some(2559));
        assert_eq!(single().right_boundary_at(0), Some(2559));
        assert_eq!(single().right_boundary_at(1439), Some(2559));
    }

    #[test]
    fn single_left_boundary() {
        assert_eq!(single().left_boundary_at(500), Some(0));
    }

    #[test]
    fn single_bottom_boundary() {
        assert_eq!(single().bottom_boundary_at(100), Some(1439));
    }

    #[test]
    fn single_top_boundary() {
        assert_eq!(single().top_boundary_at(100), Some(0));
    }

    #[test]
    fn single_out_of_range_returns_none() {
        assert_eq!(single().right_boundary_at(1440), None);
        assert_eq!(single().left_boundary_at(1440), None);
        assert_eq!(single().bottom_boundary_at(2560), None);
        assert_eq!(single().top_boundary_at(2560), None);
    }

    // ---- single monitor edge detection ----

    #[test]
    fn single_at_right_edge() {
        assert!(single().at_right_edge(2559, 500));
        assert!(!single().at_right_edge(2558, 500));
        assert!(!single().at_right_edge(2559, 1440)); // out of range y
    }

    #[test]
    fn single_at_left_edge() {
        assert!(single().at_left_edge(0, 500));
        assert!(!single().at_left_edge(1, 500));
    }

    #[test]
    fn single_at_bottom_edge() {
        assert!(single().at_bottom_edge(100, 1439));
        assert!(!single().at_bottom_edge(100, 1438));
    }

    #[test]
    fn single_at_top_edge() {
        assert!(single().at_top_edge(100, 0));
        assert!(!single().at_top_edge(100, 1));
    }

    #[test]
    fn single_crossed_edge_center_returns_none() {
        assert_eq!(single().crossed_edge(500, 500), None);
    }

    #[test]
    fn single_crossed_edge_all_sides() {
        assert_eq!(single().crossed_edge(2559, 500), Some(Edge::Right));
        assert_eq!(single().crossed_edge(0, 500),    Some(Edge::Left));
        assert_eq!(single().crossed_edge(100, 1439), Some(Edge::Bottom));
        assert_eq!(single().crossed_edge(100, 0),    Some(Edge::Top));
    }

    // ---- dual side-by-side: the key multi-monitor case ----

    #[test]
    fn dual_right_boundary_in_shared_y_region() {
        // y=500 is covered by both monitors; right boundary is DP-2's right edge
        assert_eq!(dual_side_by_side().right_boundary_at(500), Some(4479)); // 2560+1920-1
    }

    #[test]
    fn dual_right_boundary_in_dp1_only_region() {
        // y=1200 is only covered by DP-1 (DP-2 stops at y=1079)
        assert_eq!(dual_side_by_side().right_boundary_at(1200), Some(2559));
    }

    #[test]
    fn dual_left_boundary() {
        // Left boundary is always DP-1's left edge (x=0)
        assert_eq!(dual_side_by_side().left_boundary_at(500), Some(0));
        assert_eq!(dual_side_by_side().left_boundary_at(1200), Some(0));
    }

    #[test]
    fn dual_at_right_edge_dp2_region() {
        assert!(dual_side_by_side().at_right_edge(4479, 500));
        assert!(!dual_side_by_side().at_right_edge(2559, 500)); // DP-1 right edge, not the outer edge
    }

    #[test]
    fn dual_at_right_edge_dp1_only_region() {
        // At y=1200 only DP-1 exists; its right edge IS the outer boundary
        assert!(dual_side_by_side().at_right_edge(2559, 1200));
        assert!(!dual_side_by_side().at_right_edge(4479, 1200)); // not on any monitor at y=1200
    }

    #[test]
    fn dual_stacked_bottom_boundary() {
        // Both monitors share x=100; bottom is bottom of monitor 2
        assert_eq!(dual_stacked().bottom_boundary_at(100), Some(2879)); // 1440+1440-1
    }

    #[test]
    fn dual_stacked_top_boundary() {
        assert_eq!(dual_stacked().top_boundary_at(100), Some(0));
    }

    // ---- map_to_client ----

    #[test]
    fn map_right_no_offset() {
        let c = client(2560, 1600);
        assert_eq!(map_to_client(2559, 720, &c, Edge::Right, 0), (0, 720));
    }

    #[test]
    fn map_right_with_offset() {
        let c = client(2560, 1600);
        // offset=100 means client top is 100px below server top; server_y=720 -> client_y=620
        assert_eq!(map_to_client(2559, 720, &c, Edge::Right, 100), (0, 620));
    }

    #[test]
    fn map_right_clamps_below_zero() {
        let c = client(2560, 1600);
        // server_y=50, offset=200 -> -150, clamp to 0
        assert_eq!(map_to_client(2559, 50, &c, Edge::Right, 200), (0, 0));
    }

    #[test]
    fn map_right_clamps_above_height() {
        let c = client(2560, 1080);
        // server_y=1200, offset=-200 -> 1400, clamp to 1079
        assert_eq!(map_to_client(2559, 1200, &c, Edge::Right, -200), (0, 1079));
    }

    #[test]
    fn map_left_no_offset() {
        let c = client(1920, 1080);
        assert_eq!(map_to_client(0, 400, &c, Edge::Left, 0), (1919, 400));
    }

    #[test]
    fn map_bottom_no_offset() {
        let c = client(2560, 1440);
        assert_eq!(map_to_client(640, 1439, &c, Edge::Bottom, 0), (640, 0));
    }

    #[test]
    fn map_top_no_offset() {
        let c = client(2560, 1440);
        assert_eq!(map_to_client(640, 0, &c, Edge::Top, 0), (640, 1439));
    }

    #[test]
    fn map_bottom_with_offset() {
        let c = client(2560, 1440);
        // offset=100 means client left is 100px right of server left; server_x=640 -> client_x=540
        assert_eq!(map_to_client(640, 1439, &c, Edge::Bottom, 100), (540, 0));
    }

    // ---- negative monitor positions ----

    #[test]
    fn negative_origin_monitor() {
        // Monitor at (-1920, 0) -- to the left of the primary
        let layout = ServerLayout::new(vec![mon(-1920, 0, 1920, 1080), mon(0, 0, 2560, 1440)]);
        assert_eq!(layout.left_boundary_at(500), Some(-1920));
        assert_eq!(layout.right_boundary_at(500), Some(2559));
        // At y=1100 only the 2560x1440 monitor is present
        assert_eq!(layout.left_boundary_at(1100), Some(0));
    }

    // ---- empty layout ----

    #[test]
    fn empty_layout_returns_none() {
        let layout = ServerLayout::new(vec![]);
        assert_eq!(layout.right_boundary_at(0), None);
        assert_eq!(layout.crossed_edge(0, 0), None);
    }
}
