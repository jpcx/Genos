use std::fmt::Display;

use crate::gs;

pub trait Formatter {
    fn h1<T: Display>(&self, content: &T) -> String;
    fn h2<T: Display>(&self, content: &T) -> String;
    fn h3<T: Display>(&self, content: &T) -> String;
    fn text<T: Display>(&self, content: &T) -> String;
    fn bold<T: Display>(&self, content: &T) -> String;
    fn italic<T: Display>(&self, content: &T) -> String;
    fn code<T: Display>(&self, content: &T) -> String;
    fn paragraph_space(&self) -> &str;
    fn newline(&self) -> &str;
}

// this is probably buggy for bold/italic formatting of multiline text or text containing asterisks
pub struct MarkdownFormatter;

impl Formatter for MarkdownFormatter {
    fn h1<T: Display>(&self, content: &T) -> String {
        format!("# {content}")
    }

    fn h2<T: Display>(&self, content: &T) -> String {
        format!("## {content}")
    }

    fn h3<T: Display>(&self, content: &T) -> String {
        format!("### {content}")
    }

    fn text<T: Display>(&self, content: &T) -> String {
        content.to_string()
    }

    fn bold<T: Display>(&self, content: &T) -> String {
        format!("**{content}**")
    }

    fn italic<T: Display>(&self, content: &T) -> String {
        format!("*{content}*")
    }

    fn code<T: Display>(&self, content: &T) -> String {
        format!("```\n{content}\n```")
    }

    fn paragraph_space(&self) -> &str {
        "\n\n\n"
    }

    fn newline(&self) -> &str {
        "\n\n"
    }
}

impl gs::FormatType for MarkdownFormatter {
    fn format_type(&self) -> gs::TextFormat {
        gs::TextFormat::Markdown
    }
}
