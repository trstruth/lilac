use ratatui::buffer::Buffer;

pub struct Rasterizer {
    pub cell_width: u32,
    pub cell_height: u32,
}

impl Rasterizer {
    pub fn new(cell_width: u32, cell_height: u32) -> Self {
        Self {
            cell_width,
            cell_height,
        }
    }

    pub fn rasterize(
        &self,
        buffer: &Buffer,
        target_argb: &mut [u8],
        width_px: u32,
        height_px: u32,
        tick: u64,
    ) {
        // TODO: map buffer cells into pixel rectangles and blend background animation.
        let _ = (buffer, target_argb, width_px, height_px, tick);
    }
}
