use std::{cell::RefCell, fmt::Display, rc::Rc, sync::Arc};

use crate::{formatter::Formatter, points::PointQuantity, writer::Transform};

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

impl Transform for Output {
    fn transform<F: Formatter>(&self, fmt: &F) -> String {
        let mut walker = OutputWalker::new(self, fmt);
        walker.walk_transform()
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

impl<A, B> From<(A, B)> for Section
where
    A: Into<String>,
    B: Into<Content>,
{
    fn from(value: (A, B)) -> Self {
        Section::new(value.0.into()).content(value.1.into())
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
    // wrap the text in an Arc so that any clones of the RichText struct are very light.
    text: Arc<String>,
    code: bool,
}

impl RichText {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Arc::new(text.into()),
            ..Default::default()
        }
    }
}

impl Contains for RichText {
    fn contains<I: AsRef<str>>(&self, search_str: I) -> bool {
        self.text.contains(search_str.as_ref())
    }
}

impl Transform for RichText {
    fn transform<F: Formatter>(&self, fmt: &F) -> String {
        if self.code {
            fmt.code(&self.text)
        } else {
            fmt.text(&self.text)
        }
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

    pub fn update(mut self, update: Update) -> Self {
        self.updates.push(update);
        self
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
    Fail { points_lost: PointQuantity },
}

impl Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pass => write!(f, "pass"),
            Self::Fail { .. } => write!(f, "fail"),
        }
    }
}

#[derive(Clone)]
pub struct Update {
    description: String,
    status: Status,
    notes: Option<Content>,
}

impl Update {
    pub fn new_pass<D: AsRef<str>>(description: D) -> Self {
        Self {
            description: description.as_ref().to_owned(),
            status: Status::Pass,
            notes: None,
        }
    }

    pub fn new_fail<D: AsRef<str>>(description: D, points_lost: PointQuantity) -> Self {
        Self {
            description: description.as_ref().to_owned(),
            status: Status::Fail { points_lost },
            notes: None,
        }
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }

    pub fn fail(mut self, points_lost: PointQuantity) -> Self {
        self.status = Status::Fail { points_lost };
        self
    }

    pub fn set_fail(&mut self, points_lost: PointQuantity) {
        self.status = Status::Fail { points_lost };
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

struct OutputWalker<'a, F> {
    output: &'a Output,
    section_level: Rc<RefCell<u32>>,
    fmt: &'a F,
}

impl<'a, F> OutputWalker<'a, F>
where
    F: Formatter,
{
    fn new(output: &'a Output, fmt: &'a F) -> Self {
        Self {
            output,
            section_level: Rc::new(RefCell::new(0)),
            fmt,
        }
    }

    fn walk_transform(&mut self) -> String {
        let mut res = Vec::new();
        for section in &self.output.sections {
            res.push(self.section(section));
        }

        res.join(self.fmt.paragraph_space())
    }

    fn section(&mut self, section: &Section) -> String {
        let _guard = SectionLevelGuard::new(&self.section_level);
        [
            self.header(&section.header),
            self.content_list(&section.content),
        ]
        .join(self.fmt.newline())
    }

    fn header(&self, header: &String) -> String {
        match *self.section_level.borrow() {
            1 => self.fmt.h1(header),
            2 => self.fmt.h2(header),
            _ => self.fmt.h3(header),
        }
    }

    fn content_list(&mut self, content: &Vec<Content>) -> String {
        content
            .iter()
            .map(|c| self.content(c))
            .collect::<Vec<String>>()
            .join("\n\n")
    }

    fn content(&mut self, content: &Content) -> String {
        match content {
            Content::SubSection(section) => self.section(section),
            Content::Block(text) => text.transform(self.fmt),
            Content::StatusList(list) => self.status_list(list),
            Content::Multiline(content_list) => self.content_list(content_list),
        }
    }

    fn status_list(&mut self, status_list: &StatusUpdates) -> String {
        assert_ne!(
            status_list.updates.len(),
            0,
            "Expected status list to always have updates"
        );

        let summary = self.status_update_summary(status_list);
        if status_list.updates.is_empty() {
            return summary;
        }

        let feedback = status_list
            .updates
            .iter()
            .filter_map(|update| {
                update
                    .notes
                    .as_ref()
                    .map(|notes| (update.description.clone(), notes.clone()))
            })
            .map(|(desc, content)| {
                Content::SubSection((format!("feedback for {}", desc), content).into())
            })
            .collect();

        let feedback = self.content_list(&feedback);

        [summary, feedback].join(self.fmt.paragraph_space())
    }

    fn status_update_summary(&self, status_list: &StatusUpdates) -> String {
        let num_dots = 4;
        let max_len = status_list
            .updates
            .iter()
            .fold(0, |acc, update| acc.max(update.description.len()));

        status_list
            .updates
            .iter()
            .map(|update| {
                let dot_count = num_dots + max_len - update.description.len();
                let dots = std::iter::repeat(".")
                    .take(dot_count)
                    .fold(String::new(), |acc, dot| acc + dot);
                let update_str = format!("{} {} {}", update.description, dots, update.status);

                match update.status {
                    Status::Pass => update_str,
                    Status::Fail { points_lost } => {
                        format!("{} (-{})", update_str, points_lost)
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(self.fmt.newline())
    }
}

struct SectionLevelGuard {
    section_level: Rc<RefCell<u32>>,
}

impl SectionLevelGuard {
    pub fn new(level: &Rc<RefCell<u32>>) -> Self {
        *level.borrow_mut() += 1;
        Self {
            section_level: level.clone(),
        }
    }
}

impl Drop for SectionLevelGuard {
    fn drop(&mut self) {
        *self.section_level.borrow_mut() -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_into_construct() {
        let mut output = Output::new().section(
            Section::new("second header")
                .content(RichText::default())
                .content(("subsection header", RichText::default()))
                .content(("subsection header", "text"))
                .content("code here".code()),
        );
        output.add_section(Section::new("header here"));
    }

    struct MockFormatter;

    impl Formatter for MockFormatter {
        fn h1<T: Display>(&self, content: &T) -> String {
            format!("H1({})", content)
        }

        fn h2<T: Display>(&self, content: &T) -> String {
            format!("H2({})", content)
        }

        fn h3<T: Display>(&self, content: &T) -> String {
            format!("H3({})", content)
        }

        fn text<T: Display>(&self, content: &T) -> String {
            format!("{}", content)
        }

        fn bold<T: Display>(&self, content: &T) -> String {
            format!("BOLD({})", content)
        }

        fn italic<T: Display>(&self, content: &T) -> String {
            format!("ITALIC({})", content)
        }

        fn code<T: Display>(&self, content: &T) -> String {
            format!("CODE START\n{}\nCODE_END", content)
        }

        fn paragraph_space(&self) -> &str {
            "\n\n"
        }

        fn newline(&self) -> &str {
            "\n"
        }
    }

    #[test]
    fn transform_section_with_block() {
        let output = Output::new().section(("header", "Section content\nnewline"));
        let expected = "H1(header)\n\
                        Section content\n\
                        newline";
        let res = output.transform(&MockFormatter);
        assert_eq!(expected, res);
    }

    #[test]
    fn transform_section_with_code() {
        let output = Output::new().section(("header", "this is code".code()));
        let expected = "H1(header)\n\
                        CODE START\n\
                        this is code\n\
                        CODE_END";
        let res = output.transform(&MockFormatter);
        assert_eq!(expected, res);
    }

    #[test]
    fn transform_multi_section() {
        let output = Output::new()
            .section(("section 1", "section 1 content"))
            .section(("section 2", "section 2 content"));

        let expected = "H1(section 1)\n\
                        section 1 content\n\
                        \n\
                        H1(section 2)\n\
                        section 2 content";
        let res = output.transform(&MockFormatter);
        assert_eq!(expected, res);
    }

    #[test]
    fn transform_sub_section() {
        let output = Output::new()
            .section(("section 1", "section 1 content"))
            .section((
                "section 2",
                Content::SubSection(("section 3", "section 3 content").into()),
            ))
            .section(("section 4", "section 4 content"));

        let expected = "H1(section 1)\n\
                        section 1 content\n\
                        \n\
                        H1(section 2)\n\
                        H2(section 3)\n\
                        section 3 content\n\
                        \n\
                        H1(section 4)\n\
                        section 4 content";
        let res = output.transform(&MockFormatter);
        assert_eq!(expected, res);
    }

    #[test]
    fn transform_status_updates() {
        let list = StatusUpdates::default()
            .update(Update::new_pass("Section 1 long title"))
            .update(Update::new_pass("Section 2 title"))
            .update(
                Update::new_fail("Section 3", PointQuantity::Partial(2.into())).notes("Notes here"),
            )
            .update(Update::new_pass("Section 4 another long title"));
        let output = Output::new().section(("Header", Content::StatusList(list)));

        let expected = "H1(Header)\n\
                        Section 1 long title ............ pass\n\
                        Section 2 title ................. pass\n\
                        Section 3 ....................... fail (-2.00)\n\
                        Section 4 another long title .... pass\n\
                        \n\
                        H2(feedback for Section 3)\n\
                        Notes here";

        let res = output.transform(&MockFormatter);
        assert_eq!(expected, res);
    }
}
