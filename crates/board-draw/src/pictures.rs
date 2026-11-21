//! The pictures on the board's sheets. Decoding and fetching are the caller's: the renderer only
//! asks what a page's picture is right now.

use std::collections::HashMap;

/// What a sheet can show of its deck page.
#[derive(Debug, Clone)]
pub enum PictureState<I> {
    /// On its way: the sheet shows a ring.
    Loading,
    /// It cannot be had: the sheet shows a crossed frame.
    Failed,
    Ready(I),
}

pub trait Pictures<I> {
    /// Changes whenever a picture arrives or fails: part of what the board's bitmap is drawn from.
    fn revision(&self) -> u64;
    fn get(&self, deck: &str, n: u32) -> PictureState<I>;
}

/// Pictures that are all there before the board is drawn (a render has fetched what its sheets
/// need). A page that is not in the set counts as failed.
#[derive(Debug)]
pub struct StaticPictures<I> {
    revision: u64,
    images: HashMap<(String, u32), I>,
}

impl<I> Default for StaticPictures<I> {
    fn default() -> Self {
        Self {
            revision: 0,
            images: HashMap::new(),
        }
    }
}

impl<I> StaticPictures<I> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, deck: &str, n: u32, image: I) {
        self.images.insert((deck.to_owned(), n), image);
        self.revision += 1;
    }
}

impl<I: Clone> Pictures<I> for StaticPictures<I> {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn get(&self, deck: &str, n: u32) -> PictureState<I> {
        match self.images.get(&(deck.to_owned(), n)) {
            Some(image) => PictureState::Ready(image.clone()),
            None => PictureState::Failed,
        }
    }
}
