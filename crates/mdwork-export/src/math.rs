//! LaTeX math → Typst → SVG, for the live preview.

use anyhow::{anyhow, Result};
use typst_svg::{svg, SvgOptions};

use crate::pdf::compile_document;

pub struct MathOptions<'a> {
    pub fill: &'a str,
    pub size_pt: f32,
    pub display: bool,
}

impl Default for MathOptions<'_> {
    fn default() -> Self {
        Self {
            fill: "#1f2328",
            size_pt: 11.0,
            display: true,
        }
    }
}

/// Converts the LaTeX body with MiTeX and renders the equation to SVG.
pub fn math_svg(tex: &str, options: &MathOptions<'_>) -> Result<String> {
    let body = mitex::convert_math(tex, None).map_err(|err| anyhow!("MiTeX: {err}"))?;
    let body = body.trim();
    if body.is_empty() {
        return Err(anyhow!("empty formula"));
    }
    let equation = if options.display {
        format!("$ {body} $")
    } else {
        format!("#math.equation(block: false, $ {body} $)")
    };
    let source = format!(
        "#set page(width: auto, height: auto, margin: 1pt)\n\
         #set text(size: {}pt, fill: rgb(\"{}\"))\n\
         {equation}\n",
        options.size_pt, options.fill
    );
    let document = compile_document(&source)?;
    let page = document
        .pages()
        .first()
        .ok_or_else(|| anyhow!("math document has no pages"))?;
    Ok(svg(page, &SvgOptions::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_fraction_to_svg() {
        let svg = math_svg(r"\frac{1}{3}", &MathOptions::default()).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.len() > 200);
    }

    #[test]
    fn renders_inline_formula() {
        let options = MathOptions {
            display: false,
            ..Default::default()
        };
        let svg = math_svg("a^2 + b^2 = c^2", &options).unwrap();
        assert!(svg.contains("<svg"));
    }
}
