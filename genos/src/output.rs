use std::fmt::Display;

use crate::points::PointQuantity;

/*
[[ test name ]]
Points: 0/0.25
Status: Failed

[[ description ]]
some description

[[ Compile Status ]]
Running makefile on assignment.
This step uses our own makefile so make sure you don't rely on alterations to yours!

make .................. pass
some other message .... fail (-fullpoints)

a .... fail
b .... fail
ab ... fail

message describing what went wrong with additional sections

stdout:
    compile stdout

stderr:
    compile stderr

[[ Comparing output Output ]]
comparing student_stderr (diff) .... pass
comparing student_stdout (grep) .... fail (-0.25)

expected:
    1| additional details are indented
    2| and the indent continues to this line

found:
    1| student output lines

comparing outfile (grep) ........... fail (-fullpoints)

the following lines were searched for and if they weren't found, then the test was marked as wrong.

expected lines:
    line 1
    line 2
    line 3


[[ valgrind test ]]
command:
    valgrind -flag -flag ./a.out -f -d

running valgrind ..... fail

valgrind output:
    some valgrind output
    some valgrind stuff
*/

pub trait Formatter {
    fn header<T: Display>(&self, content: T);
    fn text<T: Display>(&self, content: T);
    fn code<T: Display>(&self, content: T);
}

pub trait Contains {
    fn contains<I: AsRef<str>>(&self, search_str: I) -> bool;
}

#[derive(Default, Clone)]
pub struct Output {
    sections: Vec<Section>,
}

impl Output {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn section(mut self, section: impl Into<Section>) -> Self {
        self.add_section(section);
        self
    }

    pub fn add_section(&mut self, section: impl Into<Section>) {
        self.sections.push(section.into());
    }

    pub fn append(&mut self, other: impl Into<Output>) -> &mut Self {
        self.sections.extend(other.into().sections.into_iter());
        self
    }
}

impl Contains for Output {
    fn contains<I: AsRef<str>>(&self, search_str: I) -> bool {
        let search = search_str.as_ref();
        self.sections.iter().any(|section| section.contains(search))
    }
}

#[derive(Clone)]
pub struct Section {
    header: String,
    content: Vec<Content>,
}

impl Section {
    pub fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            content: Vec::new(),
        }
    }

    pub fn content(mut self, content: impl Into<Content>) -> Self {
        self.add_content(content);
        self
    }

    pub fn add_content(&mut self, content: impl Into<Content>) {
        self.content.push(content.into());
    }
}

impl Contains for Section {
    fn contains<I: AsRef<str>>(&self, search_str: I) -> bool {
        let s = search_str.as_ref();
        self.header.contains(s) || self.content.iter().any(|content| content.contains(s))
    }
}

#[derive(Clone)]
pub enum Content {
    SubSection(Section),
    Block(RichText),
    StatusList(StatusUpdates),
    Multiline(Vec<Content>),
}

impl Contains for Content {
    fn contains<I: AsRef<str>>(&self, search_str: I) -> bool {
        match self {
            Self::SubSection(section) => section.contains(search_str),
            Self::Block(text) => text.contains(search_str),
            Self::StatusList(list) => list.contains(search_str),
            Self::Multiline(contents) => contents
                .iter()
                .any(|content| content.contains(search_str.as_ref())),
        }
    }
}

impl<A, B> Into<Content> for (A, B)
where
    A: Into<String>,
    B: Into<Content>,
{
    fn into(self) -> Content {
        Content::SubSection(Section::new(self.0.into()).content(self.1.into()))
    }
}

impl<'a> Into<Content> for &'a str {
    fn into(self) -> Content {
        Content::Block(self.into())
    }
}

impl<'a> Into<Content> for String {
    fn into(self) -> Content {
        Content::Block(self.into())
    }
}

impl Into<Content> for RichText {
    fn into(self) -> Content {
        Content::Block(self)
    }
}

impl Into<Content> for StatusUpdates {
    fn into(self) -> Content {
        Content::StatusList(self)
    }
}

#[derive(Default, Clone)]
pub struct RichText {
    text: String,
    code: bool,
}

impl RichText {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

impl Contains for RichText {
    fn contains<I: AsRef<str>>(&self, search_str: I) -> bool {
        self.text.contains(search_str.as_ref())
    }
}

pub trait RichTextMaker {
    fn code(self) -> RichText;
}

impl<T> RichTextMaker for T
where
    T: Into<String>,
{
    fn code(self) -> RichText {
        let mut text = RichText::new(self);
        text.code = true;
        text
    }
}

impl<'a> Into<RichText> for &'a str {
    fn into(self) -> RichText {
        RichText::new(self)
    }
}

impl Into<RichText> for String {
    fn into(self) -> RichText {
        RichText::new(self)
    }
}

#[derive(Clone, Default)]
pub struct StatusUpdates {
    updates: Vec<Update>,
}

impl StatusUpdates {
    pub fn add_update(&mut self, update: Update) {
        self.updates.push(update);
    }
}

impl Contains for StatusUpdates {
    fn contains<I: AsRef<str>>(&self, search_str: I) -> bool {
        self.updates
            .iter()
            .any(|update| update.contains(search_str.as_ref()))
    }
}

#[derive(Clone, Debug)]
pub enum Status {
    Pass,
    Fail,
}

#[derive(Clone)]
pub struct Update {
    description: String,
    status: Status,
    points_lost: Option<PointQuantity>,
    notes: Option<Content>,
}

impl Update {
    pub fn new<D: AsRef<str>>(description: D) -> Self {
        Self {
            description: description.as_ref().to_owned(),
            status: Status::Pass,
            points_lost: None,
            notes: None,
        }
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }

    pub fn set_status(&mut self, status: Status) {
        self.status = status;
    }

    pub fn points_lost(mut self, points: PointQuantity) -> Self {
        self.points_lost = Some(points);
        self
    }

    pub fn set_points_lost(&mut self, points: PointQuantity) {
        self.points_lost = Some(points);
    }

    pub fn notes<C: Into<Content>>(mut self, notes: C) -> Self {
        self.notes = Some(notes.into());
        self
    }

    pub fn set_notes<C: Into<Content>>(&mut self, notes: C) {
        self.notes = Some(notes.into());
    }
}

impl Contains for Update {
    fn contains<I: AsRef<str>>(&self, search_str: I) -> bool {
        self.description.contains(search_str.as_ref())
            || self
                .notes
                .as_ref()
                .map_or(false, |content| content.contains(search_str.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let mut output = Output::new().section(
            Section::new("second header")
                .content(RichText::default())
                .content(("subsection header", RichText::default()))
                .content(("subsection header", "text"))
                .content("code here".code()),
        );
        output.add_section(Section::new("header here"));
    }

    // will add more tests once this file is more finalized
}
