//! Paints a replay's board onto a stage: white paper, then the board as the player shows it.
//! Port of `BoardPainter` in `export/painters.ts`.

use std::rc::Rc;

use replay_model::{BoardReplay, MeasureText, Viewport, board_viewport};

use crate::canvas::{Canvas, Rgba};
use crate::pictures::Pictures;
use crate::render::{BoardRenderer, BoardTheme, DrawOptions};

pub struct BoardPainter<C: Canvas> {
    canvas: C,
    renderer: BoardRenderer<C>,
    measure: Rc<dyn MeasureText>,
    width: u32,
    height: u32,
}

impl<C: Canvas> std::fmt::Debug for BoardPainter<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardPainter")
            .field("size", &(self.width, self.height))
            .finish()
    }
}

impl<C: Canvas> BoardPainter<C> {
    /// A painter for a stage of `width` x `height` pixels; `like` is any canvas of the backend
    /// (it carries the fonts).
    pub fn new(like: &C, width: u32, height: u32) -> Self {
        let canvas = like.offscreen(width, height);
        let measure = canvas.measurer();
        Self {
            canvas,
            renderer: BoardRenderer::new(),
            measure,
            width,
            height,
        }
    }

    pub fn measure(&self) -> &dyn MeasureText {
        &*self.measure
    }

    /// Draws the board at `at` on `target`. The pictures of its sheets are all there before this is
    /// called (`pictures`).
    pub fn paint(
        &mut self,
        target: &mut C,
        at: (f64, f64),
        replay: &BoardReplay,
        pictures: &dyn Pictures<C::Image>,
    ) {
        let viewport = board_viewport(
            replay.scene(),
            &*self.measure,
            f64::from(self.width),
            f64::from(self.height),
        );
        self.paint_with(target, at, replay, pictures, viewport);
    }

    /// [`paint`](Self::paint) with the camera given instead of the one the replay's sheet or
    /// content asks for (the parity check draws the board at zooms a recording never has).
    pub fn paint_with(
        &mut self,
        target: &mut C,
        at: (f64, f64),
        replay: &BoardReplay,
        pictures: &dyn Pictures<C::Image>,
        viewport: Viewport,
    ) {
        let scene = replay.scene();
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        let elements = scene.ordered();
        let overlays = replay.overlays();
        self.renderer.draw(
            &mut self.canvas,
            &DrawOptions {
                elements: &elements,
                revision: scene.revision(),
                overlays: &overlays,
                hidden: &[],
                viewport,
                width: w,
                height: h,
                dpr: 1.0,
                theme: BoardTheme::Light,
                measure: &*self.measure,
            },
            pictures,
        );
        // The renderer clears to transparent: the sheet's own colour goes down first.
        target.set_fill_style(Rgba::hex(0xffffff));
        target.fill_rect(at.0, at.1, w, h);
        let image = self.canvas.image();
        target.draw_image(&image, at.0, at.1, w, h);
    }
}
