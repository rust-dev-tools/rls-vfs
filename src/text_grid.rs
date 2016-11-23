use std::ops::{Index, Add};

/// One-based line number.
#[derive(Debug, Clone, Copy)]
pub struct Line(pub u32);

/// One-based column number.
#[derive(Debug, Clone, Copy)]
pub struct Column(pub u32);

#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub start: (Line, Column),
    pub end: (Line, Column),
}

impl From<((usize, usize), (usize, usize))> for Span {
    fn from(((l1, c1), (l2, c2)): ((usize, usize), (usize, usize))) -> Self {
        fn to_u32(x: usize) -> u32 {
            assert!(x < u32::max_value() as usize);
            x as u32
        }

        Span {
            start: (Line(to_u32(l1)), Column(to_u32(c1))),
            end: (Line(to_u32(l2)), Column(to_u32(c2))),
        }
    }
}

/// Maps `Line` to byte offset.
pub struct LinesIndex(Vec<u32>);

impl LinesIndex {
    pub fn from_str(text: &str) -> Self {
        let mut result = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == 0xA {
                result.push((i + 1) as u32);
            }
        }
        result.push(text.len() as u32);
        LinesIndex(result)
    }
}

impl LinesIndex {
    pub fn get(&self, index: Line) -> Option<&u32> {
        self.0.get(index.0 as usize)
    }
}

impl Index<Line> for LinesIndex {
    type Output = u32;

    fn index(&self, index: Line) -> &u32 {
        &self.0[index.0 as usize]
    }
}

impl Add<u32> for Line {
    type Output = Line;

    fn add(self, rhs: u32) -> Line {
        Line(self.0 + rhs)
    }
}

// c is a character offset, returns a byte offset
pub fn byte_in_line(line: &str, c: Column) -> Option<usize> {
    // We simulate a null-terminated string here because spans are exclusive at
    // the top, and so that index might be outside the length of the string.
    for (i, (b, _)) in line.char_indices().chain(Some((line.len(), '\0')).into_iter()).enumerate() {
        if c.0 as usize == i {
            return Some(b);
        }
    }

    return None;
}
