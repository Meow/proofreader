//! Line breaking behind the autocorrection of `Layout/LineLength`.
//!
//! [`LineBreaker::plan`] shortens an overlong code line by turning whitespace between its tokens
//! into line breaks (and, when it unfolds an argument list spanning several lines, by
//! re-indenting the lines inside it), so the code tokens never change. It tries, in order:
//!
//! 1. breaking after the `=` of an assignment or table field, moving the value one level deeper;
//! 2. unfolding the argument list of a call or a table constructor, outermost first: one item
//!    per line one level deeper, the closing bracket on its own line;
//! 3. breaking a chain after its last `,`, `or`, `and` or `..` that fits (before the operator
//!    when the chain already starts its lines with it), or before the `:` of a chained method
//!    call; inside an `if`, `elseif` or `while` header the next line starts under the first
//!    condition.
//!
//! The first candidate whose lines all fit wins. Otherwise the first one whose remaining long
//! lines can be broken in turn wins, which is checked by simulating the next autocorrect passes
//! a few levels deep. Lines where nothing works keep their offense without a fix.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use super::alignment::char_column;
use super::block_structure::{BlockKind, BlockStructure, LineKind, indent_width};
use super::line_length::{Limits, overflow};
use super::spacing::is_callee_end;
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// How many levels of follow-up breaks [`LineBreaker::plan`] simulates for a candidate that does
/// not fit at once.
const DEPTH: usize = 3;

/// How many follow-up breaks one simulation applies at most.
const STEPS: usize = 12;

/// How many simulated passes planning one line may run in total.
const SIMULATIONS: usize = 32;

/// A rewrite of whole lines: `range` spans them, without the line break ending the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    /// The bytes replaced.
    pub range: Range<usize>,
    /// The new text of those lines.
    pub text: String,
}

/// Plans line breaks for the overlong lines of one source.
pub struct LineBreaker<'a> {
    source: &'a Source,
    limits: Limits,
    structure: BlockStructure,
    opened: HashMap<usize, usize>,
    closed: HashMap<usize, usize>,
}

/// A block or bracket pair opened on the scanned line.
#[derive(Debug, Clone, Copy)]
struct Group {
    /// Index of the block in the [`BlockStructure`].
    block: usize,
    /// Index of the opening token in [`Line::tokens`].
    open: usize,
    /// Index of the closing token in [`Line::tokens`], when it is on the line.
    close: Option<usize>,
}

/// The code of one line together with the blocks it opens and closes.
struct Line {
    number: u32,
    start: usize,
    indent: usize,
    overflow: usize,
    /// The code tokens, a trailing comment excluded.
    tokens: Vec<Token>,
    /// For every token, the groups open when it is reached, outermost first; a closing token
    /// no longer counts the group it closes, an opening token not yet the one it opens.
    enclosing: Vec<Vec<usize>>,
    groups: Vec<Group>,
    /// The groups still open at the end of the line.
    open_at_end: Vec<usize>,
    /// The first token closing a block opened on an earlier line.
    outer_close: Option<usize>,
}

impl Line {
    /// The whitespace between token `index` and the next one.
    fn gap(&self, index: usize) -> Range<usize> {
        self.tokens[index].end..self.tokens[index + 1].start
    }

    /// Whether token `index` is outside every group opened on the line.
    fn is_top(&self, index: usize) -> bool {
        self.enclosing[index].is_empty()
    }

    /// The innermost group around token `index`.
    fn innermost(&self, index: usize) -> Option<usize> {
        self.enclosing[index].last().copied()
    }
}

/// Line breaks to insert and lines to re-indent.
#[derive(Debug, Clone)]
struct Candidate {
    /// Whitespace gaps that become a line break followed by this many spaces.
    breaks: Vec<(Range<usize>, usize)>,
    /// Later lines and their new indentation.
    indents: Vec<(u32, usize)>,
    /// The last line the candidate changes.
    last: u32,
}

/// The ways of breaking one line, by strategy.
///
/// A candidate that fits at once wins, trying the assignment, the outermost unfolding and the
/// chain breaks in that order. Otherwise every unfolding (outermost first), the chain breaks and
/// the assignment are tried in that order for one whose remaining long lines can be broken in
/// later passes: moving a value that is still too long to its own line rarely helps.
struct Candidates {
    /// Strategy 1: breaking after `=`.
    assignment: Option<Candidate>,
    /// Strategy 2: unfolding an argument list or table constructor, outermost first.
    unfoldings: Vec<Candidate>,
    /// Strategy 3: breaking a chain, by operator from the loosest binding to the tightest.
    chains: Vec<Candidate>,
}

/// The `if`, `elseif` or `while` header a token belongs to.
#[derive(Debug, Clone, Copy)]
struct Header {
    /// The keyword opening the header.
    keyword: Token,
    /// The group of the header when its keyword starts the scanned line.
    group: Option<usize>,
    /// Index of the token that ends the header (`then` or `do`), or the number of tokens.
    end: usize,
}

/// Operators a chain can be broken at, from the loosest binding to the tightest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chain {
    Comma,
    Or,
    And,
    Concat,
    Method,
}

impl Chain {
    const ALL: [Chain; 5] = [
        Chain::Comma,
        Chain::Or,
        Chain::And,
        Chain::Concat,
        Chain::Method,
    ];

    /// The chain a token of this kind links.
    fn of(kind: TokenKind) -> Option<Chain> {
        match kind {
            TokenKind::Comma => Some(Chain::Comma),
            TokenKind::Or | TokenKind::OrOr => Some(Chain::Or),
            TokenKind::And | TokenKind::AndAnd => Some(Chain::And),
            TokenKind::Concat => Some(Chain::Concat),
            TokenKind::Colon => Some(Chain::Method),
            _ => None,
        }
    }

    /// Whether a line starting with a token of this kind continues this chain operator first.
    fn leads(self, kind: TokenKind) -> bool {
        match self {
            Chain::Concat => kind == TokenKind::Concat,
            Chain::Or | Chain::And => matches!(
                kind,
                TokenKind::And | TokenKind::Or | TokenKind::AndAnd | TokenKind::OrOr
            ),
            Chain::Comma | Chain::Method => false,
        }
    }
}

impl<'a> LineBreaker<'a> {
    /// Analyses `source` for breaking its lines within `limits`.
    pub fn new(source: &'a Source, limits: Limits) -> Self {
        let structure = BlockStructure::new(source);
        let mut opened = HashMap::new();
        let mut closed = HashMap::new();
        for (index, block) in structure.blocks().iter().enumerate() {
            opened.insert(block.opener.start, index);
            if let Some(closer) = block.closer {
                closed.insert(closer.start, index);
            }
        }
        LineBreaker {
            source,
            limits,
            structure,
            opened,
            closed,
        }
    }

    /// The rewrite that brings line `number` within the maximum length, if one is found.
    pub fn plan(&self, number: u32) -> Option<Rewrite> {
        self.plan_at(number, DEPTH, &Cell::new(SIMULATIONS))
    }

    /// [`LineBreaker::plan`], simulating at most `depth` levels of follow-up breaks.
    fn plan_at(&self, number: u32, depth: usize, budget: &Cell<usize>) -> Option<Rewrite> {
        let line = self.scan(number)?;
        let Candidates {
            assignment,
            unfoldings,
            chains,
        } = self.candidates(&line);
        let max = self.limits.max;
        let immediate = assignment
            .iter()
            .chain(unfoldings.first())
            .chain(chains.iter());
        for candidate in immediate {
            let rewrite = self.render(&line, candidate);
            if rewrite.text.split('\n').all(|text| fits(text, max)) {
                return Some(rewrite);
            }
        }
        let level = depth.checked_sub(1)?;
        unfoldings
            .iter()
            .chain(chains.iter())
            .chain(assignment.iter())
            .map(|candidate| self.render(&line, candidate))
            .find(|rewrite| {
                rewrite
                    .text
                    .split('\n')
                    .next()
                    .is_some_and(|text| fits(text, max))
                    && self.settles(rewrite, number, level, budget)
            })
    }

    /// Whether, after applying `rewrite` to the lines starting at `first`, the next passes can
    /// break every resulting line that is still too long, simulating `depth` levels.
    fn settles(&self, rewrite: &Rewrite, first: u32, depth: usize, budget: &Cell<usize>) -> bool {
        let mut text = splice(&self.source.text, rewrite);
        let mut last = first.saturating_add(line_breaks(&rewrite.text));
        for _ in 0..STEPS {
            let Some(left) = budget.get().checked_sub(1) else {
                return false;
            };
            budget.set(left);
            let source = Source::new(self.source.path.clone(), text);
            let Some(long) =
                (first..=last).find(|&number| overflow(&source, number, &self.limits).is_some())
            else {
                return true;
            };
            let Some(next) = LineBreaker::new(&source, self.limits).plan_at(long, depth, budget)
            else {
                return false;
            };
            let removed = line_breaks(&source.text[next.range.clone()]);
            last = last
                .max(long.saturating_add(removed))
                .saturating_add(line_breaks(&next.text))
                .saturating_sub(removed);
            text = splice(&source.text, &next);
        }
        false
    }

    /// The code of line `number`, unless the line cannot be broken: it holds no code, starts or
    /// ends inside a multi-line token, is indented with tabs, contains padding between tokens
    /// (usually alignment with its neighbours), or overflows only because of a trailing comment.
    fn scan(&self, number: u32) -> Option<Line> {
        let source = self.source;
        if !matches!(
            self.structure.kind(number),
            LineKind::Statement | LineKind::Continuation | LineKind::Closer
        ) {
            return None;
        }
        let range = source.line_range(number);
        let text = &source.text[range.clone()];
        if text.ends_with('\r') {
            return None;
        }
        let overflow = range.start + text.char_indices().nth(self.limits.max)?.0;
        let indent = indent_width(source, number)?;
        let mut tokens = Vec::new();
        let mut comment = None;
        for token in source
            .tokens_on_line(number)
            .iter()
            .filter(|token| !token.is_trivia() && token.kind != TokenKind::Eof)
        {
            if token.is_multiline() || comment.is_some() || token.kind == TokenKind::Unknown {
                return None;
            }
            if token.kind.is_comment() {
                comment = Some(*token);
            } else {
                tokens.push(*token);
            }
        }
        let last = tokens.last()?;
        if comment.is_some() && last.end <= overflow {
            return None;
        }
        let padded = tokens
            .iter()
            .chain(comment.iter())
            .zip(tokens.iter().skip(1).chain(comment.iter()))
            .any(|(before, after)| !matches!(&source.text[before.end..after.start], "" | " "));
        if padded {
            return None;
        }
        let mut groups: Vec<Group> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut enclosing = Vec::with_capacity(tokens.len());
        let mut outer_close = None;
        for (index, token) in tokens.iter().enumerate() {
            if let Some(&block) = self.closed.get(&token.start) {
                match stack
                    .iter()
                    .rposition(|&group| groups[group].block == block)
                {
                    Some(position) => {
                        groups[stack[position]].close = Some(index);
                        stack.truncate(position);
                    }
                    None => {
                        let segment =
                            index == 0 && matches!(token.kind, TokenKind::ElseIf | TokenKind::Else);
                        if !segment {
                            outer_close.get_or_insert(index);
                        }
                    }
                }
            }
            enclosing.push(stack.clone());
            if let Some(&block) = self.opened.get(&token.start) {
                groups.push(Group {
                    block,
                    open: index,
                    close: None,
                });
                stack.push(groups.len() - 1);
            }
        }
        Some(Line {
            number,
            start: range.start,
            indent,
            overflow,
            tokens,
            enclosing,
            groups,
            open_at_end: stack,
            outer_close,
        })
    }

    /// Every way of breaking `line`, by strategy, that keeps the alignment of nearby lines.
    fn candidates(&self, line: &Line) -> Candidates {
        let partners = self.alignment_partners(line);
        let keeps_alignment =
            |candidate: &Candidate| !self.moves_aligned_token(line, candidate, &partners);
        Candidates {
            assignment: self.after_assignment(line).filter(keeps_alignment),
            unfoldings: self
                .unfoldings(line)
                .into_iter()
                .filter(keeps_alignment)
                .collect(),
            chains: self
                .chain_breaks(line)
                .into_iter()
                .filter(keeps_alignment)
                .collect(),
        }
    }

    /// Strategy 1: the value of an assignment or table field that ends on this line moves to
    /// the next line, one level deeper.
    fn after_assignment(&self, line: &Line) -> Option<Candidate> {
        if self.structure.kind(line.number) != LineKind::Statement
            || line.outer_close.is_some()
            || !line.open_at_end.is_empty()
            || self
                .next_code_line(line.number)
                .is_some_and(|next| self.structure.kind(next) == LineKind::Continuation)
        {
            return None;
        }
        let assign = (0..line.tokens.len())
            .find(|&index| line.is_top(index) && line.tokens[index].kind == TokenKind::Assign)?;
        (assign + 1 < line.tokens.len()).then(|| Candidate {
            breaks: vec![(line.gap(assign), line.indent + self.limits.width)],
            indents: Vec::new(),
            last: line.number,
        })
    }

    /// Strategy 2: the argument lists and table constructors opened on this line that hold the
    /// overrun, outermost first, unfolded to one item per line.
    fn unfoldings(&self, line: &Line) -> Vec<Candidate> {
        if self.outer_header(line).is_some() {
            return Vec::new();
        }
        let limit = line.outer_close.unwrap_or(line.tokens.len());
        let mut order: Vec<usize> = (0..line.groups.len())
            .filter(|&group| self.is_unfoldable(line, group, limit))
            .collect();
        for position in 1..order.len() {
            let (outer, inner) = (order[position - 1], order[position]);
            if self.is_last_table_argument(line, inner, outer) {
                order.swap(position - 1, position);
            }
        }
        order
            .into_iter()
            .filter_map(|group| self.unfold(line, group))
            .collect()
    }

    /// Whether `inner` is a table constructor passed as the last argument of the call `outer`
    /// on the same line, which unfolds before the call (`f(a, {` ... `})`).
    fn is_last_table_argument(&self, line: &Line, inner: usize, outer: usize) -> bool {
        let (table, call) = (line.groups[inner], line.groups[outer]);
        self.structure.block(table.block).kind == BlockKind::Brace
            && line.innermost(table.open) == Some(outer)
            && (table.open - 1 == call.open || line.tokens[table.open - 1].kind == TokenKind::Comma)
            && table
                .close
                .is_some_and(|close| call.close == Some(close + 1))
    }

    /// Whether `group` is a call's argument list or a table constructor outside any keyword
    /// block of the line that opens before the limit and holds the overrun.
    fn is_unfoldable(&self, line: &Line, group: usize, limit: usize) -> bool {
        let Group { block, open, close } = line.groups[group];
        let kind = self.structure.block(block).kind;
        let opener = line.tokens[open];
        let call = kind == BlockKind::Paren
            && open
                .checked_sub(1)
                .is_some_and(|before| is_callee_end(line.tokens[before].kind));
        (call || kind == BlockKind::Brace)
            && open < limit
            && open + 1 < line.tokens.len()
            && close != Some(open + 1)
            && line.enclosing[open].iter().all(|&outer| {
                self.structure
                    .block(line.groups[outer].block)
                    .kind
                    .is_bracket()
            })
            && opener.end <= line.overflow
            && close.is_none_or(|close| {
                line.tokens[close].end > line.overflow
                    && line
                        .tokens
                        .get(close + 1)
                        .is_none_or(|next| ends_operand_chain(next.kind))
            })
    }

    /// The candidate unfolding `group`; `None` when the lines after it cannot follow.
    fn unfold(&self, line: &Line, group: usize) -> Option<Candidate> {
        let Group { open, close, .. } = line.groups[group];
        let item_indent = line.indent + self.limits.width;
        let end = close.unwrap_or(line.tokens.len());
        let mut breaks = vec![(line.gap(open), item_indent)];
        breaks.extend(
            (open + 1..end)
                .filter(|&index| {
                    line.tokens[index].kind == TokenKind::Comma
                        && line.innermost(index) == Some(group)
                        && index + 1 < end
                })
                .map(|index| (line.gap(index), item_indent)),
        );
        match close {
            Some(close) => {
                breaks.push((line.gap(close - 1), line.indent));
                Some(Candidate {
                    breaks,
                    indents: Vec::new(),
                    last: line.number,
                })
            }
            None => self.unfold_across_lines(line, group, breaks),
        }
    }

    /// Completes the unfolding of `group`, which closes on a later line: the lines up to its
    /// closer shift with the items of this line, and the closer moves to a line of its own.
    ///
    /// The line must end with a comma between two items, or with the opening of a block
    /// inside the last item whose body starts on the next line (`function(a)`, `{`).
    fn unfold_across_lines(
        &self,
        line: &Line,
        group: usize,
        mut breaks: Vec<(Range<usize>, usize)>,
    ) -> Option<Candidate> {
        let source = self.source;
        let closer = self.structure.block(line.groups[group].block).closer?;
        let last_index = line.tokens.len() - 1;
        let width = self.limits.width;
        let delta = if line.tokens[last_index].kind == TokenKind::Comma
            && line.innermost(last_index) == Some(group)
        {
            let next = self
                .next_code_line(line.number)
                .filter(|&next| next <= closer.line)?;
            isize::try_from(line.indent + width).ok()?
                - isize::try_from(indent_width(source, next)?).ok()?
        } else {
            let position = line.open_at_end.iter().position(|&open| open == group)?;
            let &innermost = line.open_at_end.last()?;
            let block = self.structure.block(line.groups[innermost].block);
            if position + 1 == line.open_at_end.len()
                || !block.opens_line
                || block.body_start != Some(line.tokens[last_index])
            {
                return None;
            }
            isize::try_from(width).ok()?
        };
        let skipped = source.lines_inside_multiline_tokens();
        let closer_first = source.first_code_token_on_line(closer.line) == Some(&closer);
        let mut indents = Vec::new();
        for number in line.number + 1..=closer.line {
            if skipped.contains(&number) || source.is_blank(number) {
                continue;
            }
            if number == closer.line && closer_first {
                indents.push((number, line.indent));
                continue;
            }
            if self.has_padding(number) {
                return None;
            }
            let current = indent_width(source, number)?;
            let target = current.checked_add_signed(delta)?;
            if target != current {
                indents.push((number, target));
            }
        }
        if !closer_first {
            let before = source
                .tokens_on_line(closer.line)
                .iter()
                .rev()
                .filter(|token| token.start < closer.start)
                .find(|token| !token.is_trivia())?;
            if before.kind.is_comment() || before.end_line != closer.line {
                return None;
            }
            breaks.push((before.end..closer.start, line.indent));
        }
        let keeps_blocks_open = self.structure.blocks().iter().any(|block| {
            block.opener.line == closer.line
                && block.opener.start > closer.start
                && block.closer.is_none_or(|end| end.line > closer.line)
        });
        if keeps_blocks_open && indent_width(source, closer.line) != Some(line.indent) {
            return None;
        }
        Some(Candidate {
            breaks,
            indents,
            last: closer.line,
        })
    }

    /// Strategy 3: for each kind of chain operator, from the loosest binding to the tightest,
    /// the break at its last operator outside the brackets of the line that keeps this line
    /// within the maximum.
    fn chain_breaks(&self, line: &Line) -> Vec<Candidate> {
        let tokens = &line.tokens;
        let own = self
            .own_header(line)
            .filter(|header| self.may_become_multiline(line, header));
        let outer = self.outer_header(line);
        if !line
            .open_at_end
            .iter()
            .all(|&group| own.is_some_and(|header| header.group == Some(group)))
        {
            return Vec::new();
        }
        let limit = line
            .outer_close
            .unwrap_or(tokens.len())
            .min(outer.map_or(tokens.len(), |header| header.end));
        let assign =
            (0..limit).find(|&index| line.is_top(index) && tokens[index].kind == TokenKind::Assign);
        let next = self
            .next_code_line(line.number)
            .filter(|&next| self.structure.kind(next) == LineKind::Continuation);
        let mut best: Vec<(Chain, Range<usize>, usize)> = Vec::new();
        for index in assign.map_or(0, |assign| assign + 1)..limit {
            let header = own
                .filter(|header| {
                    header
                        .group
                        .is_some_and(|group| line.enclosing[index] == [group])
                        && index < header.end
                })
                .or(outer.filter(|_| line.is_top(index)));
            if !line.is_top(index) && header.is_none() {
                continue;
            }
            let Some(chain) = Chain::of(tokens[index].kind) else {
                continue;
            };
            let leading = self.leading_indent(line, chain, next);
            let gap_index = match chain {
                Chain::Comma if header.is_some() => continue,
                Chain::Method => {
                    let method = index >= 1
                        && tokens[index - 1].kind == TokenKind::RParen
                        && tokens
                            .get(index + 1)
                            .is_some_and(|name| name.kind == TokenKind::Name);
                    if !method {
                        continue;
                    }
                    index - 1
                }
                _ if leading.is_some() => match index.checked_sub(1) {
                    Some(before) if assign != Some(before) => before,
                    _ => continue,
                },
                _ if index + 1 < tokens.len() => index,
                _ => continue,
            };
            let gap = line.gap(gap_index);
            if !fits(&self.source.text[line.start..gap.start], self.limits.max) {
                continue;
            }
            let indent = match (leading, header) {
                (Some(indent), _) => indent,
                (None, Some(header)) => self
                    .condition_column(header)
                    .unwrap_or_else(|| self.continuation_indent(line)),
                (None, None) if chain == Chain::Comma && self.in_bracket(line) => line.indent,
                (None, None) => self.continuation_indent(line),
            };
            best.retain(|(kind, _, _)| *kind != chain);
            best.push((chain, gap, indent));
        }
        Chain::ALL
            .iter()
            .filter_map(|chain| best.iter().find(|(kind, _, _)| kind == chain))
            .map(|(_, gap, indent)| Candidate {
                breaks: vec![(gap.clone(), *indent)],
                indents: Vec::new(),
                last: line.number,
            })
            .collect()
    }

    /// The header opened by the keyword starting the line.
    fn own_header(&self, line: &Line) -> Option<Header> {
        let keyword = line.tokens[0];
        if !matches!(
            keyword.kind,
            TokenKind::If | TokenKind::ElseIf | TokenKind::While
        ) {
            return None;
        }
        let group = line.groups.iter().position(|group| group.open == 0)?;
        let block = self.structure.block(line.groups[group].block);
        Some(Header {
            keyword,
            group: Some(group),
            end: self.header_end(line, block.body_start),
        })
    }

    /// The header of an `if`, `elseif` or `while` begun on an earlier line that this
    /// continuation line belongs to.
    fn outer_header(&self, line: &Line) -> Option<Header> {
        if self.structure.kind(line.number) != LineKind::Continuation {
            return None;
        }
        let block = self.structure.block(self.structure.innermost(line.number)?);
        let header = matches!(
            block.kind,
            BlockKind::If | BlockKind::ElseIf | BlockKind::While
        ) && block
            .body_start
            .is_none_or(|token| token.start >= line.start);
        header.then(|| Header {
            keyword: block.opener,
            group: None,
            end: self.header_end(line, block.body_start),
        })
    }

    /// Index in the line of the token ending a header, or the number of tokens.
    fn header_end(&self, line: &Line, body_start: Option<Token>) -> usize {
        body_start
            .and_then(|token| line.tokens.iter().position(|other| *other == token))
            .unwrap_or(line.tokens.len())
    }

    /// Whether breaking the header makes a one-line block multi-line without
    /// `Layout/EmptyLineBeforeBlock` asking for a blank line before it.
    fn may_become_multiline(&self, line: &Line, header: &Header) -> bool {
        let one_line = header
            .group
            .is_some_and(|group| line.groups[group].close.is_some());
        if !one_line {
            return true;
        }
        let number = line.number;
        let previous = number - 1;
        number == 1
            || matches!(
                self.structure.kind(previous),
                LineKind::Blank | LineKind::Comment
            )
            || !self.structure.is_statement_start(number)
            || match self.structure.innermost(number) {
                Some(block) => self.structure.block(block).first_body_line == Some(number),
                None => (1..number).all(|before| !self.structure.has_code(before)),
            }
    }

    /// The column of the first condition token after the keyword of `header`, when it is on the
    /// keyword's line.
    fn condition_column(&self, header: Header) -> Option<usize> {
        let source = self.source;
        let indexes = source.code_token_indexes();
        let position =
            indexes.partition_point(|&index| source.tokens[index].start <= header.keyword.start);
        let first = source.tokens[*indexes.get(position)?];
        (first.line == header.keyword.end_line && !first.kind.is_comment())
            .then(|| char_column(source, &first))
    }

    /// The indentation of a chain that already puts `chain` operators first: that of this line
    /// when it starts with one, or of the continuation line `next` when it does.
    fn leading_indent(&self, line: &Line, chain: Chain, next: Option<u32>) -> Option<usize> {
        if chain.leads(line.tokens[0].kind) {
            return Some(line.indent);
        }
        let next = next?;
        self.structure
            .first_token(next)
            .filter(|first| chain.leads(first.kind))
            .and_then(|_| indent_width(self.source, next))
    }

    /// The indentation of a new continuation line: one level deeper than the statement, and at
    /// least as deep as this line when it is a continuation already.
    fn continuation_indent(&self, line: &Line) -> usize {
        let width = self.limits.width;
        if self.structure.kind(line.number) != LineKind::Continuation {
            return line.indent + width;
        }
        let statement = indent_width(self.source, self.structure.statement_line(line.number))
            .unwrap_or(line.indent);
        line.indent.max(statement + width)
    }

    /// Whether the line starts inside a bracket opened on an earlier line.
    fn in_bracket(&self, line: &Line) -> bool {
        self.structure
            .innermost(line.number)
            .is_some_and(|block| self.structure.block(block).kind.is_bracket())
    }

    /// The first line after `number` holding code.
    fn next_code_line(&self, number: u32) -> Option<u32> {
        (number + 1..=self.structure.line_count()).find(|&next| self.structure.has_code(next))
    }

    /// Whether line `number` has more than one space, or a tab, between two of its tokens.
    fn has_padding(&self, number: u32) -> bool {
        let tokens: Vec<&Token> = self.source.code_tokens_on_line(number).collect();
        tokens.windows(2).any(|pair| {
            pair[0].end_line == number
                && !matches!(&self.source.text[pair[0].end..pair[1].start], "" | " ")
        })
    }

    /// The columns of the line that padded tokens on the lines it is adjacent to for alignment
    /// purposes line up with: the nearest code line above and below, and the nearest ones with
    /// the same indentation (as `Layout/ExtraSpacing` looks at them).
    fn alignment_partners(&self, line: &Line) -> HashSet<usize> {
        let source = self.source;
        let indentation = source.indentation(line.number);
        let code = |number: &u32| {
            source
                .code_tokens_on_line(*number)
                .any(|token| !token.kind.is_comment())
        };
        let same_indentation = |number: &u32| source.indentation(*number) == indentation;
        let above = || (1..line.number).rev().filter(code);
        let below = || (line.number + 1..=source.line_count()).filter(code);
        let neighbours = [
            above().next(),
            below().next(),
            above().find(same_indentation),
            below().find(same_indentation),
        ];
        let mut columns = HashSet::new();
        for number in neighbours.into_iter().flatten() {
            let tokens: Vec<&Token> = source.code_tokens_on_line(number).collect();
            for pair in tokens.windows(2) {
                if pair[0].end_line == number
                    && !matches!(&source.text[pair[0].end..pair[1].start], "" | " ")
                {
                    columns.insert(char_column(source, pair[1]));
                }
            }
        }
        columns
    }

    /// Whether `candidate` moves a token of the line that a padded token nearby lines up with.
    fn moves_aligned_token(
        &self,
        line: &Line,
        candidate: &Candidate,
        partners: &HashSet<usize>,
    ) -> bool {
        let end = self.source.line_range(line.number).end;
        let Some(first) = candidate
            .breaks
            .iter()
            .map(|(gap, _)| gap.start)
            .filter(|&start| start < end)
            .min()
        else {
            return false;
        };
        line.tokens
            .iter()
            .filter(|token| token.start >= first)
            .any(|token| partners.contains(&char_column(self.source, token)))
    }

    /// The text of the lines `candidate` changes, with its breaks and indentation applied.
    fn render(&self, line: &Line, candidate: &Candidate) -> Rewrite {
        let source = self.source;
        let range = line.start..source.line_range(candidate.last).end;
        let mut edits: Vec<(Range<usize>, String)> = candidate
            .breaks
            .iter()
            .map(|(gap, indent)| (gap.clone(), format!("\n{}", " ".repeat(*indent))))
            .collect();
        edits.extend(candidate.indents.iter().map(|&(number, indent)| {
            let start = source.line_range(number).start;
            (
                start..start + source.indentation(number).len(),
                " ".repeat(indent),
            )
        }));
        edits.sort_by_key(|(range, _)| range.start);
        let mut text = String::with_capacity(range.len() + 64);
        let mut cursor = range.start;
        for (edit, replacement) in edits {
            text.push_str(&source.text[cursor..edit.start]);
            text.push_str(&replacement);
            cursor = edit.end;
        }
        text.push_str(&source.text[cursor..range.end]);
        Rewrite { range, text }
    }
}

/// Whether a token of this kind may follow the closing bracket of an unfolded list on its line:
/// another closer, a separator, or an index or call on the result. A binary operator or a method
/// call (which chain breaks handle) may not.
fn ends_operand_chain(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::RParen
            | TokenKind::RBrace
            | TokenKind::RBracket
            | TokenKind::Comma
            | TokenKind::Semicolon
            | TokenKind::Dot
            | TokenKind::LBracket
            | TokenKind::LParen
    )
}

/// Whether `text` is at most `max` characters long.
fn fits(text: &str, max: usize) -> bool {
    text.chars().nth(max).is_none()
}

/// The number of line breaks in `text`.
fn line_breaks(text: &str) -> u32 {
    u32::try_from(text.matches('\n').count()).unwrap_or(u32::MAX)
}

/// `text` with `rewrite` applied.
fn splice(text: &str, rewrite: &Rewrite) -> String {
    let mut result = String::with_capacity(text.len() + rewrite.text.len());
    result.push_str(&text[..rewrite.range.start]);
    result.push_str(&rewrite.text);
    result.push_str(&text[rewrite.range.end..]);
    result
}
