//! The Breakout grid: how many columns and rows for N surfaces, and each cell's rectangle.

/// A rectangle in screen points, origin at the top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// `(columns, rows)` for `count` surfaces, or `None` outside 1 to 9.
pub fn grid_dims(count: usize) -> Option<(usize, usize)> {
    match count {
        1 => Some((1, 1)),
        2 => Some((2, 1)),
        3 => Some((3, 1)),
        4 => Some((2, 2)),
        5 | 6 => Some((3, 2)),
        7..=9 => Some((3, 3)),
        _ => None,
    }
}

/// One rectangle per surface, row by row, filling `screen`.
pub fn tile(count: usize, screen: Rect) -> Option<Vec<Rect>> {
    let (cols, rows) = grid_dims(count)?;
    let cell_w = screen.width / cols as u32;
    let cell_h = screen.height / rows as u32;
    Some(
        (0..count)
            .map(|index| {
                let (col, row) = (index % cols, index / cols);
                // The last column and row absorb the division remainder.
                let width = if col == cols - 1 {
                    screen.width - cell_w * (cols as u32 - 1)
                } else {
                    cell_w
                };
                let height = if row == rows - 1 {
                    screen.height - cell_h * (rows as u32 - 1)
                } else {
                    cell_h
                };
                Rect {
                    x: screen.x + (cell_w * col as u32) as i32,
                    y: screen.y + (cell_h * row as u32) as i32,
                    width,
                    height,
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0,
        y: 38,
        width: 1728,
        height: 1079,
    };

    #[test]
    fn dims_follow_the_spec_table() {
        for (count, want) in [
            (0, None),
            (1, Some((1, 1))),
            (2, Some((2, 1))),
            (3, Some((3, 1))),
            (4, Some((2, 2))),
            (5, Some((3, 2))),
            (6, Some((3, 2))),
            (7, Some((3, 3))),
            (9, Some((3, 3))),
            (10, None),
        ] {
            assert_eq!(grid_dims(count), want, "{count}");
        }
    }

    #[test]
    fn two_surfaces_split_the_width() {
        let cells = tile(2, SCREEN).unwrap();
        assert_eq!(
            cells,
            vec![
                Rect {
                    x: 0,
                    y: 38,
                    width: 864,
                    height: 1079
                },
                Rect {
                    x: 864,
                    y: 38,
                    width: 864,
                    height: 1079
                },
            ]
        );
    }

    #[test]
    fn cells_cover_the_screen_without_overlap_or_gaps() {
        for count in 1..=9 {
            let cells = tile(count, SCREEN).unwrap();
            assert_eq!(cells.len(), count);
            let (cols, _) = grid_dims(count).unwrap();
            // Each full row's widths sum to the screen width, remainder included.
            let first_row: u32 = cells.iter().take(cols.min(count)).map(|c| c.width).sum();
            assert_eq!(first_row, SCREEN.width, "{count}");
            let last = cells.last().unwrap();
            assert!(last.y as u32 + last.height <= SCREEN.y as u32 + SCREEN.height);
        }
    }

    #[test]
    fn five_surfaces_leave_the_last_cell_empty() {
        let cells = tile(5, SCREEN).unwrap();
        assert_eq!(cells[3].y, cells[4].y, "second row");
        assert_eq!(cells[3].x, 0);
        assert_eq!(cells[4].x, 576);
    }

    #[test]
    fn a_screen_with_an_offset_origin_is_respected() {
        let second = Rect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let cells = tile(4, second).unwrap();
        assert_eq!(
            cells[0],
            Rect {
                x: -1920,
                y: 0,
                width: 960,
                height: 540
            }
        );
        assert_eq!(
            cells[3],
            Rect {
                x: -960,
                y: 540,
                width: 960,
                height: 540
            }
        );
    }
}
