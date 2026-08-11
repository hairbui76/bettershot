//! Everything that can go wrong on the way from a [`crate::Canvas`] to a file.
//!
//! Rendering itself is deliberately infallible: a `Painter` call can be a
//! no-op (off-canvas, degenerate, non-finite) but it can never fail, because
//! there is no sensible way for an annotation to "fail to draw" halfway
//! through an export. Only I/O, codecs and resource loading produce errors.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    #[error("{width}x{height} RGBA8 needs {expected} bytes, got {actual}")]
    BufferSize {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },

    #[error("image dimensions {width}x{height} do not fit in memory on this platform")]
    TooLarge { width: u32, height: u32 },

    #[error("i/o error for `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("image codec error: {0}")]
    Codec(#[from] image::ImageError),

    /// No font could be found in any of the candidate locations. Only
    /// [`crate::Font::try_system`] reports this; [`crate::Font::system`]
    /// downgrades it to a warning and returns the block-glyph fallback so an
    /// export never dies over a missing font.
    #[error("no usable system font found (searched {searched} candidate paths)")]
    FontNotFound { searched: usize },

    #[error("`{path}` could not be parsed as a font")]
    FontInvalid { path: PathBuf },
}

impl RenderError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        RenderError::Io {
            path: path.into(),
            source,
        }
    }
}
