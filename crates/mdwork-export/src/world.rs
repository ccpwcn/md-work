use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};

fn file_id(path: &str) -> FileId {
    let vpath = VirtualPath::new(path).expect("invalid virtual path");
    RootedPath::new(VirtualRoot::Project, vpath).intern()
}
use typst_kit::fonts::{self, FontStore};

static FONTS: LazyLock<FontStore> = LazyLock::new(|| {
    let mut store = FontStore::new();
    // Bundled fonts first so math and code always have a complete face set,
    // then the system fonts for CJK and user preferences.
    store.extend(fonts::embedded());
    store.extend(fonts::system());
    store
});

pub struct ExportWorld {
    main: FileId,
    sources: HashMap<FileId, Source>,
    files: HashMap<FileId, Bytes>,
    library: LazyHash<Library>,
}

impl ExportWorld {
    pub fn new(main_source: impl Into<String>) -> Self {
        let main = file_id("main.typ");
        let source = Source::new(main, main_source.into());
        let mut sources = HashMap::new();
        sources.insert(main, source);
        Self {
            main,
            sources,
            files: HashMap::new(),
            library: LazyHash::new(Library::default()),
        }
    }

    pub fn from_doc(doc: &mdwork_render::typst::TypstDoc) -> Self {
        let mut world = Self::new(doc.source.clone());
        for asset in &doc.assets {
            world.add_file(&asset.name, asset.bytes.clone());
        }
        world
    }

    pub fn add_file(&mut self, path: &str, bytes: impl Into<Vec<u8>>) {
        self.files.insert(file_id(path), Bytes::new(bytes.into()));
    }

    pub fn source_text(&self) -> &str {
        self.sources
            .get(&self.main)
            .map(|source| source.text())
            .unwrap_or_default()
    }

    fn missing(id: FileId) -> FileError {
        FileError::NotFound(PathBuf::from(id.vpath().get_without_slash()))
    }
}

impl World for ExportWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        FONTS.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.sources
            .get(&id)
            .cloned()
            .ok_or_else(|| Self::missing(id))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files
            .get(&id)
            .cloned()
            .ok_or_else(|| Self::missing(id))
    }

    fn font(&self, index: usize) -> Option<Font> {
        FONTS.font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}
