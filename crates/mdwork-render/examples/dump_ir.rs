fn main() {
    let md = std::fs::read_to_string(r"D:\Documents\md-test\新笔记本\d1\d1note.md").unwrap();
    let ir = mdwork_render::preview::PreviewIr::from_markdown(&md);
    println!("blocks={}", ir.blocks.len());
    for (i, b) in ir.blocks.iter().enumerate() {
        match b {
            mdwork_render::preview::Block::Heading {
                level,
                content,
                line,
            } => {
                let t: String = content
                    .iter()
                    .filter_map(|c| {
                        if let mdwork_render::preview::Inline::Text(x) = c {
                            Some(x.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                println!("{i}: H{level} L{line} {t:?}");
            }
            mdwork_render::preview::Block::Paragraph { content, line } => {
                println!(
                    "{i}: P L{line} n={} {:?}",
                    content.len(),
                    content
                        .iter()
                        .map(|c| match c {
                            mdwork_render::preview::Inline::Text(t) => format!("T({t})"),
                            mdwork_render::preview::Inline::Image { src, .. } =>
                                format!("Img({src})"),
                            mdwork_render::preview::Inline::Span { text, .. } =>
                                format!("S({text})"),
                            mdwork_render::preview::Inline::SoftBreak => "SB".into(),
                            _ => "?".into(),
                        })
                        .collect::<Vec<_>>()
                );
            }
            mdwork_render::preview::Block::Image {
                src, width, line, ..
            } => println!("{i}: Img L{line} {src} w={width:?}"),
            mdwork_render::preview::Block::Div { text, line, .. } => {
                println!("{i}: Div L{line} {text:?}")
            }
            _ => println!("{i}: other"),
        }
    }
}
