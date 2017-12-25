#![deny(warnings)]

extern crate arg_parser;
extern crate extra;

use std::cell::Cell; // Provide mutable fields in immutable structs
use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, BufReader, Read, Stderr, StdoutLock, Write};
use std::process::exit;
use extra::option::OptionalExt;
use arg_parser::ArgParser;

const MAN_PAGE: &'static str = /* @MANSTART{cat} */ r#"NAME
    cat - concatenate files and print on the standard output

SYNOPSIS
    cat [-h | --help] [-A | --show-all] [-b | --number-nonblank] [-e] [-E | --show-ends]
        [-n | --number] [-s | --squeeze-blank] [-t] [-T] FILES...

DESCRIPTION
    Concatenates all files to the standard output.

    If no file is given, or if FILE is '-', read from standard input.

OPTIONS
    -A
    --show-all
        equivalent to -vET

    -b
    --number-nonblank
        number nonempty output lines, overriding -n

    -e
        equivalent to -vE

    -E
    --show-ends
        display $ at the end of each line

    -n
    --number
        number all output lines

    -s
    --squeeze-blank
        supress repeated empty output lines

    -t
        equivalent to -vT

    -T
    --show_tabs
        display TAB characters as ^I

    -v
    --show-nonprinting
        use caret (^) and M- notation, except for LFD and TAB.

    -h
    --help
        display this help and exit

AUTHOR
    Written by Michael Murphy.
"#; /* @MANEND */

struct Program {
    exit_status:      Cell<i32>,
    number:           bool,
    number_nonblank:  bool,
    show_ends:        bool,
    show_tabs:        bool,
    show_nonprinting: bool,
    squeeze_blank:    bool,
    paths:            Vec<String>,
}

impl Program {
    /// Initialize the program's arguments and flags.
    fn initialize(stdout: &mut StdoutLock, stderr: &mut Stderr) -> Program {
        let mut parser = ArgParser::new(10).
            add_flag(&["A", "show-all"]). //vET
            add_flag(&["b", "number-nonblank"]).
            add_flag(&["e"]). //vE
            add_flag(&["E", "show-ends"]).
            add_flag(&["n", "number"]).
            add_flag(&["s", "squeeze-blank"]).
            add_flag(&["t"]). //vT
            add_flag(&["T", "show-tabs"]).
            add_flag(&["v", "show-nonprinting"]).
            add_flag(&["h", "help"]);
        parser.parse(env::args());

        let mut cat = Program {
            exit_status:      Cell::new(0i32),
            number:           false,
            number_nonblank:  false,
            show_ends:        false,
            show_tabs:        false,
            show_nonprinting: false,
            squeeze_blank:    false,
            paths:            Vec::with_capacity(parser.args.len()),
        };

        if parser.found("help") {
            stdout.write(MAN_PAGE.as_bytes()).try(stderr);
            stdout.flush().try(stderr);
            exit(0);
        }

        if parser.found("show-all") {
            cat.show_nonprinting = true;
            cat.show_ends = true;
            cat.show_tabs = true;
        }

        if parser.found("number") {
            cat.number = true;
            cat.number_nonblank = false;
        }

        if parser.found("number-nonblank") {
            cat.number_nonblank = true;
            cat.number = false;
        }

        if parser.found("show-ends") || parser.found(&'e') {
            cat.show_ends = true;
        }

        if parser.found("squeeze-blank") {
            cat.squeeze_blank = true;
        }

        if parser.found("show-tabs") || parser.found(&'t') {
            cat.show_tabs = true;
        }

        if parser.found("show-nonprinting") || parser.found(&'e') || parser.found(&'t') {
            cat.show_nonprinting = true;
        }

        if !parser.args.is_empty() {
            cat.paths = parser.args;
        }
        cat
    }

    /// Execute the parameters given to the program.
    fn and_execute(&self, stdout: &mut StdoutLock, stderr: &mut Stderr) -> i32 {
        let stdin = io::stdin();
        let line_count = &mut 0usize;
        let flags_enabled = self.number || self.number_nonblank || self.show_ends || self.show_tabs ||
                            self.squeeze_blank || self.show_nonprinting;

        if self.paths.is_empty() && flags_enabled {
            self.cat(&mut stdin.lock(), line_count, stdout, stderr);
        } else if self.paths.is_empty() {
            self.simple_cat(&mut stdin.lock(), stdout, stderr);
        } else {
            for path in &self.paths {
                if flags_enabled && path == "-" {
                    self.cat(&mut stdin.lock(), line_count, stdout, stderr);
                } else if path == "-" {
                    // Copy the standard input directly to the standard output.
                    self.simple_cat(&mut stdin.lock(), stdout, stderr);
                } else if fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
                    stderr.write(path.as_bytes()).try(stderr);
                    stderr.write(b": Is a directory\n").try(stderr);
                    stderr.flush().try(stderr);
                    self.exit_status.set(1i32);
                } else if flags_enabled {
                    fs::File::open(&path)
                        // Open the file and copy the file's contents to standard output based input arguments.
                        .map(|file| self.cat(&mut BufReader::new(file), line_count, stdout, stderr))
                        // If an error occurred, print the error and set the exit status.
                        .unwrap_or_else(|message| {
                            stderr.write(path.as_bytes()).try(stderr);
                            stderr.write(b": ").try(stderr);
                            stderr.write(message.description().as_bytes()).try(stderr);
                            stderr.write(b"\n").try(stderr);
                            stderr.flush().try(stderr);
                            self.exit_status.set(1i32);
                        });
                } else {
                    // Open a file and copy the contents directly to standard output.
                    fs::File::open(&path).map(|ref mut file| { self.simple_cat(file, stdout, stderr); })
                        // If an error occurs, print the error and set the exit status.
                        .unwrap_or_else(|message| {
                            stderr.write(path.as_bytes()).try(stderr);
                            stderr.write(b": ").try(stderr);
                            stderr.write(message.description().as_bytes()).try(stderr);
                            stderr.write(b"\n").try(stderr);
                            stderr.flush().try(stderr);
                            self.exit_status.set(1i32);
                        });
                }
            }
        }
        self.exit_status.get()
    }

    /// A simple cat that runs a lot faster than self.cat() due to no iterators over single bytes.
    fn simple_cat<F: Read>(&self, file: &mut F, stdout: &mut StdoutLock, stderr: &mut Stderr) { 
        let mut buf: [u8; 8*8192] = [0; 8*8192]; // 64K seems to be the sweet spot for a buffer on my machine.
        loop { 
            let n_read = file.read(&mut buf).try(stderr);
            if n_read == 0 { // We've reached the end of the input
                break;
            }
            stdout.write_all(&buf[..n_read]).try(stderr);
        }
    }

    /// Cats either a file or stdin based on the flag arguments given to the program.
    fn cat<F: Read>(&self, file: &mut F, line_count: &mut usize, stdout: &mut StdoutLock, stderr: &mut Stderr) {
        let mut character_count = 0;
        let mut last_line_was_blank = false;
        let mut buf: [u8; 8*8192] = [0; 8*8192]; // 64K seems to be the sweet spot for a buffer on my machine.
        let mut out_buf: Vec<u8> = Vec::with_capacity(24*8192); // Worst case 2 chars out per char
        loop { 
            let n_read = file.read(&mut buf).try(stderr);
            if n_read == 0 { // We've reached the end of the input
                break;
            }

            for &byte in buf[0..n_read].iter() {
                if character_count == 0 && (self.number || (self.number_nonblank && byte != b'\n')) {
                    out_buf.write(b"     ").try(stderr);
                    out_buf.write(line_count.to_string().as_bytes()).try(stderr);
                    out_buf.write(b"  ").try(stderr);
                    *line_count += 1;
                }
                match byte {
                    0...8 | 11...31 => if self.show_nonprinting {
                        push_caret(&mut out_buf, stderr, byte+64);
                        count_character(&mut character_count, &self.number, &self.number_nonblank);
                    },
                    9 => {
                        if self.show_tabs {
                            push_caret(&mut out_buf, stderr, b'I');
                        } else {
                            out_buf.write(&[byte]).try(stderr);
                        }
                        count_character(&mut character_count, &self.number, &self.number_nonblank);
                    }
                    10 => {
                        if character_count == 0 {
                            if self.squeeze_blank && last_line_was_blank {
                                continue
                            } else if !last_line_was_blank {
                                last_line_was_blank = true;
                            }
                        } else {
                            last_line_was_blank = false;
                            character_count = 0;
                        }
                        if self.show_ends {
                            out_buf.write(b"$\n").try(stderr);
                        } else {
                            out_buf.write(b"\n").try(stderr);
                        }
                    },
                    32...126 => {
                        out_buf.write(&[byte]).try(stderr);
                        count_character(&mut character_count, &self.number, &self.number_nonblank);
                    },
                    127 => if self.show_nonprinting {
                        push_caret(&mut out_buf, stderr, b'?');
                        count_character(&mut character_count, &self.number, &self.number_nonblank);
                    },
                    128...159 => if self.show_nonprinting {
                        out_buf.write(b"M-^").try(stderr);
                        out_buf.write(&[byte-64]).try(stderr);
                        count_character(&mut character_count, &self.number, &self.number_nonblank);
                    } else {
                        out_buf.write(&[byte]).try(stderr);
                        count_character(&mut character_count, &self.number, &self.number_nonblank);
                    },
                    _ => if self.show_nonprinting {
                        out_buf.write(b"M-").try(stderr);
                        out_buf.write(&[byte-128]).try(stderr);
                        count_character(&mut character_count, &self.number, &self.number_nonblank);
                    } else {
                        out_buf.write(&[byte]).try(stderr);
                        count_character(&mut character_count, &self.number, &self.number_nonblank);
                    },
                }
            }
            stdout.write_all(&out_buf).try(stderr);
            out_buf.clear();
        }
    }
}
/// Increase the character count by one if number printing is enabled.
fn count_character(character_count: &mut usize, number: &bool, number_nonblank: &bool) {
    if *number || *number_nonblank {
        *character_count += 1;
    }
}

/// Print a caret notation to stdout.
fn push_caret<T: Write>(stdout: &mut T, stderr: &mut Stderr, notation: u8) {
    stdout.write(&[b'^']).try(stderr);
    stdout.write(&[notation]).try(stderr);
}

fn main() {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut stderr = io::stderr();
    exit(Program::initialize(&mut stdout, &mut stderr).and_execute(&mut stdout, &mut stderr));
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::process::Output;
    use count_character;

    #[test]
    fn count_character_number_lines() {
        let character_count: &mut usize = &mut 0;

        count_character(character_count, &true, &false);
        assert_eq!(character_count, &mut 1);
    }

    #[test]
    fn count_character_number_none_empty_lines() {
        let character_count: &mut usize = &mut 0;

        count_character(character_count, &false, &true);
        assert_eq!(character_count, &mut 1);
    }

    #[test]
    fn count_character_number_lines_and_none_blank_lines() {
        let character_count: &mut usize = &mut 0;

        count_character(character_count, &true, &true);
        assert_eq!(character_count, &mut 1);
    }

    #[test]
    fn count_character_number_no_lines() {
        let character_count: &mut usize = &mut 0;

        count_character(character_count, &false, &false);
        assert_eq!(character_count, &mut 0);
    }

    fn run_cat_command(arguments: &[&str]) -> Output {
        return Command::new("target/debug/cat")
            .args(arguments)
            .output()
            .expect("Failed to execute command");
    }

    #[test]
    fn none_empty_text_file() {
        let output = run_cat_command(&["testing/file_with_text"]);

        assert!(&output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), String::from("FILE IS NOT EMPTY\n"));
    }

    #[test]
    fn empty_text_file() {
        let output = run_cat_command(&["testing/empty_file"]);

        assert!(&output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), String::from(String::from("")));
    }

    #[test]
    fn empty_executable_file() {
        let output = run_cat_command(&["testing/empty_executable _file"]);

        assert!(&output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), String::from(String::from("")));
    }

    #[test]
    fn none_empty_executable_file() {
        let output = run_cat_command(&["testing/executable_file"]);

        assert!(&output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), String::from(String::from("FILE IS NOT EMPTY.\n")));
    }

    #[test]
    fn multi_line_lang_file() {
        let output = run_cat_command(&["testing/multi_line_lang_file"]);

        let correct_result = "Hello, 世界.\n\nThis is a file with\nseveral lines and".to_owned() +
            &" some of them are\n\nempty or with trailing or\u{85}funny\u{a0}spaces.".to_owned() +
            &" \n\nPangrams in different languages:\nЖълтата дюля беше щастлива, че пухът, който".to_owned() +
            &" цъфна, замръзна като гьон.\nΓαζέες καὶ μυρτιὲς δὲν θὰ βρῶ πιὰ στὸ χρυσαφὶ ".to_owned() +
            &"ξέφωτο\nいろはにほへとちりぬるを\n? דג סקרן שט בים מאוכזב ולפתע מצא לו חברה איך הקליטה\nPijamalı".to_owned() +
            &" hasta, yağız şoföre çabucak güvendi.\n\n🦀\n";

        assert!(&output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), String::from(correct_result));
    }

    #[test]
    fn empty_symlink() {
        let output = run_cat_command(&["testing/symlink"]);

        assert!(&output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), String::from(""));
    }

    #[test]
    fn none_existent_file() {
        let output = run_cat_command(&["testing/none_existent_file"]);

        assert!(String::from_utf8_lossy(&output.stdout).is_empty());
        assert!(!&output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("entity not found"));
    }
}