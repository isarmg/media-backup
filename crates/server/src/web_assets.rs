//! The bytes served by the binary are also the release's verified Web contract.
pub(crate) const HTML: &str = include_str!("../../../clients/web/dist/index.html");
pub(crate) const SCRIPT: &[u8] = include_bytes!("../../../clients/web/dist/assets/admin.js");
pub(crate) const STYLES: &[u8] = include_bytes!("../../../clients/web/dist/assets/admin.css");
pub(crate) const FONT: &[u8] = include_bytes!("../../../clients/web/dist/assets/MapleMono.woff2");
pub(crate) const ITALIC_FONT: &[u8] =
    include_bytes!("../../../clients/web/dist/assets/MapleMono-Italic.woff2");
pub(crate) const FONT_LICENSE: &[u8] =
    include_bytes!("../../../clients/web/dist/assets/MapleMono-OFL.txt");

pub(crate) const RELEASE_FILES: &[(&str, &[u8])] = &[
    ("share/web/assets/MapleMono-Italic.woff2", ITALIC_FONT),
    ("share/web/assets/MapleMono-OFL.txt", FONT_LICENSE),
    ("share/web/assets/MapleMono.woff2", FONT),
    ("share/web/assets/admin.css", STYLES),
    ("share/web/assets/admin.js", SCRIPT),
    ("share/web/index.html", HTML.as_bytes()),
];
