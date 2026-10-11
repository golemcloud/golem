//! Local commands that take part of their work from a bound tool: each test gives the session a
//! stand-in for the tool that changes the bytes, so that a test can tell the work of the stand-in
//! from the work of the command.
use std::path::{Path, PathBuf};

use super::Session;

/// Runs `script` in a fresh session that `prepare` has set up. Returns the status, stdout and
/// stderr.
fn run(script: &str, prepare: impl FnOnce(&mut Session)) -> (u8, String, String) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(tokio::task::LocalSet::new().run_until(async {
        let mut session = Session::new_with_execution_services(crate::ExecutionServices::default())
            .await
            .unwrap();
        prepare(&mut session);
        session.run(script).await
    }));
    (
        result.exit_code,
        String::from_utf8(result.stdout).unwrap(),
        String::from_utf8(result.stderr).unwrap(),
    )
}

/// A new directory for one test, with the file `f` of two lines.
fn directory(group: &str, name: &str) -> PathBuf {
    let directory = Path::new("/tmp/bash-forwarded").join(group).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("f"), "one\ntwo\n").unwrap();
    directory
}

mod read {
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    use crate::commands::FileReader;

    /// What the file reader of a test answers.
    #[derive(Clone, Copy)]
    enum Answer {
        /// The content of the file in upper case, so that a test can tell it from a local read.
        UpperCase,
        /// Nothing: the command must read the file itself.
        Nothing,
        /// One byte less than the file has.
        TooShort,
    }

    struct Reader {
        answer: Answer,
        asked: Mutex<Vec<(PathBuf, u64)>>,
    }

    impl FileReader for Reader {
        fn read(&self, path: &Path, len: u64) -> Option<Vec<u8>> {
            self.asked.lock().unwrap().push((path.to_owned(), len));
            let content = std::fs::read(path).ok()?;
            match self.answer {
                Answer::UpperCase => Some(content.to_ascii_uppercase()),
                Answer::Nothing => None,
                Answer::TooShort => Some(content[..content.len() - 1].to_vec()),
            }
        }
    }

    /// Runs `script` in a fresh session with a file reader that gives `answer`. Returns the
    /// status, stdout, stderr and each read the reader was asked for.
    fn run(script: &str, answer: Answer) -> (u8, String, String, Vec<(PathBuf, u64)>) {
        let reader = Arc::new(Reader {
            answer,
            asked: Mutex::default(),
        });
        let (status, stdout, stderr) = super::run(script, |session| {
            session.forward_file_reads(reader.clone());
        });
        let asked = reader.asked.lock().unwrap().clone();
        (status, stdout, stderr, asked)
    }

    fn directory(name: &str) -> PathBuf {
        super::directory("read", name)
    }

    #[test]
    fn cat_prints_what_the_file_reader_returns_for_a_regular_file() {
        let directory = directory("regular");
        let (status, stdout, stderr, asked) =
            run(&format!("cat {}/f", directory.display()), Answer::UpperCase);
        assert_eq!(
            (status, stdout.as_str(), stderr.as_str()),
            (0, "ONE\nTWO\n", "")
        );
        assert_eq!(asked, [(directory.join("f"), 8)]);
    }

    #[test]
    fn cat_formats_what_the_file_reader_returns() {
        let directory = directory("format");
        let (status, stdout, _, asked) = run(
            &format!("cd {}; cat -n f; cat -A f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(
            (status, stdout.as_str()),
            (0, "     1\tONE\n     2\tTWO\nONE$\nTWO$\n")
        );
        assert_eq!(asked.len(), 2);
    }

    #[test]
    fn cat_asks_the_file_reader_with_the_absolute_path_of_a_relative_operand() {
        let directory = directory("relative");
        let (_, stdout, _, asked) = run(
            &format!("cd {}; cat f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(stdout, "ONE\nTWO\n");
        assert_eq!(asked, [(directory.join("f"), 8)]);
    }

    #[test]
    fn cat_keeps_its_message_for_a_missing_operand_between_forwarded_ones() {
        let directory = directory("missing");
        let (status, stdout, stderr, asked) = run(
            &format!("cd {}; cat f missing f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(
            (status, stdout.as_str(), stderr.as_str()),
            (
                1,
                "ONE\nTWO\nONE\nTWO\n",
                "cat: missing: No such file or directory\n"
            )
        );
        assert_eq!(asked.len(), 2);
    }

    #[test]
    fn cat_reads_the_file_itself_when_the_file_reader_returns_nothing() {
        let directory = directory("nothing");
        let (status, stdout, stderr, asked) =
            run(&format!("cat {}/f", directory.display()), Answer::Nothing);
        assert_eq!(
            (status, stdout.as_str(), stderr.as_str()),
            (0, "one\ntwo\n", "")
        );
        assert_eq!(asked.len(), 1);
    }

    #[test]
    fn cat_reads_the_file_itself_when_the_file_reader_returns_another_length() {
        let directory = directory("length");
        let (status, stdout, stderr, asked) =
            run(&format!("cat {}/f", directory.display()), Answer::TooShort);
        assert_eq!(
            (status, stdout.as_str(), stderr.as_str()),
            (0, "one\ntwo\n", "")
        );
        assert_eq!(asked.len(), 1);
    }

    /// Standard input under each of its names, a named descriptor, the null device, an empty
    /// file and a directory are not regular files with content.
    #[test]
    fn cat_does_not_ask_the_file_reader_for_what_is_not_a_regular_file_with_content() {
        let directory = directory("other");
        std::fs::write(directory.join("empty"), "").unwrap();
        let (status, stdout, stderr, asked) = run(
            &format!(
                "cd {}; cat < f; cat - < f; cat /dev/stdin < f; cat /dev/fd/3 3< f; \
                 cat /dev/null; cat empty; cat .",
                directory.display()
            ),
            Answer::UpperCase,
        );
        assert_eq!(asked, []);
        assert_eq!(stdout, "one\ntwo\n".repeat(4));
        assert_eq!((status, stderr.as_str()), (1, "cat: .: Is a directory\n"));
    }

    #[test]
    fn cat_does_not_ask_the_file_reader_when_its_input_is_its_output() {
        let directory = directory("same");
        let (status, stdout, stderr, asked) = run(
            &format!("cd {}; cat f >> f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(asked, []);
        assert_eq!(
            (status, stdout.as_str(), stderr.as_str()),
            (1, "", "cat: f: input file is output file\n")
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("f")).unwrap(),
            "one\ntwo\n"
        );
    }

    /// A read through the file reader is not a wait: the stage ends before its reader exits.
    #[test]
    fn cat_with_a_file_reader_ends_before_a_reader_that_exits_early() {
        let directory = directory("early");
        let (_, stdout, _, asked) = run(
            &format!(
                "set -o pipefail; cat {}/f | head -n 0; echo \"${{PIPESTATUS[*]}}\"",
                directory.display()
            ),
            Answer::UpperCase,
        );
        assert_eq!(stdout, "0 0\n");
        assert_eq!(asked.len(), 1);
    }
}

mod write {
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    use crate::commands::FileWriter;

    /// What the file writer of a test does with a file.
    #[derive(Clone, Copy)]
    enum Answer {
        /// Writes the content in upper case, so that a test can tell it from a local write.
        UpperCase,
        /// Writes nothing and refuses: the command must write the file itself.
        Refuse,
        /// Writes other bytes and then refuses, as a write that failed part of the way.
        Spoil,
        /// Writes one byte less than the content and says that it wrote the file.
        TooShort,
    }

    struct Writer {
        answer: Answer,
        max_len: usize,
        asked: Mutex<Vec<(PathBuf, String)>>,
    }

    impl FileWriter for Writer {
        fn max_len(&self) -> usize {
            self.max_len
        }

        fn write(&self, path: &Path, content: &[u8]) -> bool {
            self.asked.lock().unwrap().push((
                path.to_owned(),
                String::from_utf8_lossy(content).into_owned(),
            ));
            match self.answer {
                Answer::UpperCase => std::fs::write(path, content.to_ascii_uppercase()).is_ok(),
                Answer::Refuse => false,
                Answer::Spoil => {
                    let _ = std::fs::write(path, "other content, and more of it");
                    false
                }
                Answer::TooShort => std::fs::write(path, &content[..content.len() - 1]).is_ok(),
            }
        }
    }

    /// The outcome of a script, and each write that the file writer was asked for.
    struct Outcome {
        status: u8,
        stdout: String,
        stderr: String,
        asked: Vec<(PathBuf, String)>,
    }

    /// Runs `script` in a fresh session with a file writer that takes `max_len` bytes and does
    /// `answer`.
    fn run_limited(script: &str, answer: Answer, max_len: usize) -> Outcome {
        let writer = Arc::new(Writer {
            answer,
            max_len,
            asked: Mutex::default(),
        });
        let (status, stdout, stderr) = super::run(script, |session| {
            session.forward_file_writes(writer.clone());
        });
        let asked = writer.asked.lock().unwrap().clone();
        Outcome {
            status,
            stdout,
            stderr,
            asked,
        }
    }

    fn run(script: &str, answer: Answer) -> Outcome {
        run_limited(script, answer, 1024 * 1024)
    }

    fn directory(name: &str) -> PathBuf {
        super::directory("write", name)
    }

    fn content(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn tee_gives_the_content_of_a_file_to_the_file_writer() {
        let directory = directory("regular");
        let out = directory.join("out");
        let outcome = run(
            &format!("printf 'one\\ntwo\\n' | tee {}", out.display()),
            Answer::UpperCase,
        );
        assert_eq!(
            (
                outcome.status,
                outcome.stdout.as_str(),
                outcome.stderr.as_str()
            ),
            (0, "one\ntwo\n", "")
        );
        assert_eq!(outcome.asked, [(out.clone(), "one\ntwo\n".to_owned())]);
        assert_eq!(content(&out), "ONE\nTWO\n");
    }

    #[test]
    fn tee_asks_the_file_writer_with_the_absolute_path_of_a_relative_operand() {
        let directory = directory("relative");
        let outcome = run(
            &format!("cd {}; echo hi | tee out", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(outcome.stdout, "hi\n");
        assert_eq!(outcome.asked, [(directory.join("out"), "hi\n".to_owned())]);
    }

    #[test]
    fn tee_gives_each_file_operand_to_the_file_writer_in_order() {
        let directory = directory("several");
        let outcome = run(
            &format!("cd {}; echo hi | tee b a", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!((outcome.status, outcome.stdout.as_str()), (0, "hi\n"));
        assert_eq!(
            outcome.asked,
            [
                (directory.join("b"), "hi\n".to_owned()),
                (directory.join("a"), "hi\n".to_owned())
            ]
        );
        assert_eq!(content(&directory.join("a")), "HI\n");
        assert_eq!(content(&directory.join("b")), "HI\n");
    }

    #[test]
    fn tee_replaces_the_content_of_a_file_that_exists() {
        let directory = directory("replace");
        let outcome = run(
            &format!("cd {}; echo hi | tee f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(outcome.asked.len(), 1);
        assert_eq!(content(&directory.join("f")), "HI\n");
    }

    #[test]
    fn tee_writes_the_file_itself_when_the_file_writer_refuses() {
        let directory = directory("refuse");
        let outcome = run(
            &format!("cd {}; echo hi | tee f", directory.display()),
            Answer::Refuse,
        );
        assert_eq!(
            (
                outcome.status,
                outcome.stdout.as_str(),
                outcome.stderr.as_str()
            ),
            (0, "hi\n", "")
        );
        assert_eq!(outcome.asked.len(), 1);
        assert_eq!(content(&directory.join("f")), "hi\n");
    }

    #[test]
    fn tee_writes_the_file_again_when_the_file_writer_left_other_content() {
        let directory = directory("spoil");
        let outcome = run(
            &format!("cd {}; echo hi | tee f", directory.display()),
            Answer::Spoil,
        );
        assert_eq!((outcome.status, outcome.stderr.as_str()), (0, ""));
        assert_eq!(outcome.asked.len(), 1);
        assert_eq!(content(&directory.join("f")), "hi\n");
    }

    #[test]
    fn tee_writes_the_file_again_when_the_file_writer_wrote_another_length() {
        let directory = directory("length");
        let outcome = run(
            &format!("cd {}; echo hi | tee f", directory.display()),
            Answer::TooShort,
        );
        assert_eq!((outcome.status, outcome.stderr.as_str()), (0, ""));
        assert_eq!(outcome.asked.len(), 1);
        assert_eq!(content(&directory.join("f")), "hi\n");
    }

    #[test]
    fn tee_does_not_ask_the_file_writer_for_an_append() {
        let directory = directory("append");
        let outcome = run(
            &format!("cd {}; echo three | tee -a f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(outcome.asked, []);
        assert_eq!((outcome.status, outcome.stdout.as_str()), (0, "three\n"));
        assert_eq!(content(&directory.join("f")), "one\ntwo\nthree\n");
    }

    /// A standard stream under each of its names and the null device are not regular files.
    #[test]
    fn tee_does_not_ask_the_file_writer_for_what_is_not_a_regular_file() {
        let outcome = run(
            "echo hi | tee /dev/stderr /dev/null /dev/stdout /dev/fd/2",
            Answer::UpperCase,
        );
        assert_eq!(outcome.asked, []);
        assert_eq!(
            (
                outcome.status,
                outcome.stdout.as_str(),
                outcome.stderr.as_str()
            ),
            (0, "hi\nhi\n", "hi\nhi\n")
        );
    }

    #[test]
    fn tee_with_no_input_makes_an_empty_file_and_does_not_ask_the_file_writer() {
        let directory = directory("empty");
        let outcome = run(
            &format!("cd {}; tee f new < /dev/null", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(outcome.asked, []);
        assert_eq!((outcome.status, outcome.stdout.as_str()), (0, ""));
        assert_eq!(content(&directory.join("f")), "");
        assert_eq!(content(&directory.join("new")), "");
    }

    #[test]
    fn tee_keeps_its_message_for_a_file_that_it_cannot_open() {
        let directory = directory("cannot-open");
        let outcome = run(
            &format!("cd {}; echo hi | tee missing/x f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(
            (
                outcome.status,
                outcome.stdout.as_str(),
                outcome.stderr.as_str()
            ),
            (1, "hi\n", "tee: missing/x: No such file or directory\n")
        );
        assert_eq!(outcome.asked, [(directory.join("f"), "hi\n".to_owned())]);
    }

    /// The file writer takes a file only as a whole, and a file must have the bytes that `tee`
    /// has taken when another part of the script runs. So `tee` writes what it has before it
    /// waits, and then it goes on with the file itself.
    #[test]
    fn tee_writes_the_file_itself_before_it_waits_for_more_input() {
        let directory = directory("wait");
        let outcome = run(
            &format!(
                "cd {}; {{ echo first; sleep 0.05; cat out; echo last; }} | tee out",
                directory.display()
            ),
            Answer::UpperCase,
        );
        assert_eq!(outcome.asked, []);
        assert_eq!(
            (outcome.status, outcome.stdout.as_str()),
            (0, "first\nfirst\nlast\n")
        );
        assert_eq!(content(&directory.join("out")), "first\nfirst\nlast\n");
    }

    /// With nothing taken yet, a wait leaves nothing to write, and the file writer still gets
    /// the file.
    #[test]
    fn tee_gives_the_file_to_the_file_writer_after_a_wait_with_nothing_taken() {
        let directory = directory("wait-first");
        let outcome = run(
            &format!(
                "cd {}; {{ sleep 0.05; echo late; }} | tee out",
                directory.display()
            ),
            Answer::UpperCase,
        );
        assert_eq!(outcome.stdout, "late\n");
        assert_eq!(
            outcome.asked,
            [(directory.join("out"), "late\n".to_owned())]
        );
        assert_eq!(content(&directory.join("out")), "LATE\n");
    }

    /// A write through the file writer is not a wait: the next stage sees the file as written.
    #[test]
    fn tee_with_a_file_writer_ends_before_the_next_stage_runs() {
        let directory = directory("next-stage");
        let outcome = run(
            &format!("cd {}; echo hi | tee out | cat out", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!((outcome.status, outcome.stdout.as_str()), (0, "HI\n"));
        assert_eq!(outcome.asked.len(), 1);
    }

    #[test]
    fn tee_gives_the_file_writer_content_up_to_its_limit() {
        let directory = directory("limit");
        let outcome = run_limited(
            &format!("cd {}; printf 12345678 | tee out", directory.display()),
            Answer::UpperCase,
            8,
        );
        assert_eq!(outcome.asked.len(), 1);
        let outcome = run_limited(
            &format!("cd {}; printf 12345678a | tee out", directory.display()),
            Answer::UpperCase,
            8,
        );
        assert_eq!(outcome.asked, []);
        assert_eq!((outcome.status, outcome.stdout.as_str()), (0, "12345678a"));
        assert_eq!(content(&directory.join("out")), "12345678a");
    }

    /// A later output that is a pipe with no reader ends `tee` at once, with no wait after the
    /// bytes that it took for the file before.
    #[test]
    fn tee_keeps_what_it_took_when_a_later_output_ends_it() {
        let directory = directory("later-output");
        let script = format!(
            "cd {}; {{ sleep 0.05; echo hi; }} | tee out /dev/stderr 2>&1 >/dev/null | true; \
             echo \"${{PIPESTATUS[*]}}\"; cat out",
            directory.display()
        );
        let local = super::run(&script, |_| ());
        assert_eq!(local, (0, "0 141 0\nhi\n".to_owned(), String::new()));
        let outcome = run(&script, Answer::UpperCase);
        assert_eq!(outcome.asked, []);
        assert_eq!((outcome.status, outcome.stdout, outcome.stderr), local);
    }

    /// `tee` ends when its standard output is a pipe that has no reader. The file then has the
    /// bytes that `tee` took before.
    #[test]
    fn tee_keeps_what_it_took_when_its_reader_exits() {
        let directory = directory("reader-exits");
        let outcome = run(
            &format!(
                "cd {}; {{ echo first; sleep 0.05; echo second; sleep 0.05; echo third; }} \
                 | tee out | head -n 1; echo \"${{PIPESTATUS[*]}}\"",
                directory.display()
            ),
            Answer::UpperCase,
        );
        assert_eq!(outcome.asked, []);
        assert_eq!(outcome.stdout, "first\n141 141 0\n");
        assert_eq!(content(&directory.join("out")), "first\n");
    }
}
