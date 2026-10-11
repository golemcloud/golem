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
        /// The content of the file as it is.
        Exact,
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
                Answer::Exact => Some(content),
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

    /// Runs `script` two times in a new directory `name` that has the text file `big` of
    /// 204,800 bytes, which is more than a pipe holds: with no file reader, and with one that
    /// gives a file as it is. Asserts that the status and the output are the same, and that
    /// the file reader was asked one time. Returns the standard output.
    fn same_as_without_a_file_reader(name: &str, script: &str) -> String {
        let run_in_new_directory = |answer: Option<Answer>| {
            let directory = directory(name);
            std::fs::write(directory.join("big"), "0123456789abcde\n".repeat(12_800)).unwrap();
            let script = format!("cd {}; {script}", directory.display());
            match answer {
                Some(answer) => run(&script, answer),
                None => {
                    let (status, stdout, stderr) = super::run(&script, |_| ());
                    (status, stdout, stderr, Vec::new())
                }
            }
        };
        let (status, stdout, stderr, _) = run_in_new_directory(None);
        let forwarded = run_in_new_directory(Some(Answer::Exact));
        assert_eq!(
            (forwarded.0, &forwarded.1, &forwarded.2),
            (status, &stdout, &stderr),
            "{script}"
        );
        assert_eq!(forwarded.3.len(), 1, "{script}");
        stdout
    }

    /// `cat` reads a file piece by piece, and it waits when its reader does not take the
    /// bytes. Another part of the script can change the file in that time, and `cat` then
    /// reads what the file has. Here the next stage empties the file while `cat` waits.
    #[test]
    fn cat_reads_the_rest_of_the_file_itself_after_it_waits() {
        let stdout =
            same_as_without_a_file_reader("wait", "cat big | tee big | wc -c; wc -c < big");
        assert_ne!(stdout, "204800\n204800\n");
    }

    /// After a wait, `cat` goes on in the file at the byte after the last one that it took.
    /// Here another part of the script puts other content of the same size in the file while
    /// `cat` waits. `cat` then writes the size of the file only if it goes on at that byte.
    #[test]
    fn cat_goes_on_at_its_place_in_the_file_after_it_waits() {
        let stdout = same_as_without_a_file_reader(
            "place",
            "cksum < big; cat big | { sleep 0.05; printf '%204799s\\n' '' > big; cksum; }",
        );
        let sums = stdout.lines().collect::<Vec<_>>();
        assert!(
            matches!(sums[..], [before, written] if before != written && written.ends_with(" 204800")),
            "{stdout}"
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
        /// Writes the content as it is.
        Exact,
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
                Answer::Exact => std::fs::write(path, content).is_ok(),
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

    /// Input from a here-string and from a file arrives with no wait, as input from a writer
    /// that has ended does.
    #[test]
    fn tee_gives_input_that_is_ready_to_the_file_writer() {
        let directory = directory("ready");
        let outcome = run(
            &format!("cd {}; tee a <<< here; tee b < f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(
            (outcome.status, outcome.stdout.as_str()),
            (0, "here\none\ntwo\n")
        );
        assert_eq!(
            outcome.asked,
            [
                (directory.join("a"), "here\n".to_owned()),
                (directory.join("b"), "one\ntwo\n".to_owned())
            ]
        );
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

    /// Runs `script` in a new directory `name` two times: with no file writer, and with one
    /// that writes the content as it is. Asserts that the status and the output are the same.
    /// Returns what the file writer was asked for.
    fn same_as_without_a_file_writer(name: &str, script: &str) -> Vec<(PathBuf, String)> {
        let script = |directory: &Path| format!("cd {}; {script}", directory.display());
        let local = super::run(&script(&directory(name)), |_| ());
        let outcome = run(&script(&directory(name)), Answer::Exact);
        assert_eq!(
            (outcome.status, outcome.stdout, outcome.stderr),
            local,
            "{}",
            script(Path::new(name))
        );
        outcome.asked
    }

    /// The file writer makes the content of the file at a path. `tee` writes to the file that
    /// it has opened, at the start of that file. The two are the same only while the path
    /// names that file and the file is empty. Another part of the script can change that
    /// while `tee` waits with no bytes for the file.
    #[test]
    fn tee_writes_the_file_itself_when_the_file_changed_before_its_input_came() {
        for change in [
            "printf 'xxxxxxxxxx\\n' > out",
            "echo more >> out",
            "rm out",
            // Another file at the same path, which is also empty.
            "rm out; : > out",
            // The same, with the modification time of the first file: a host can give such a
            // file the identity of the first one.
            "touch -r out time; rm out; : > out; touch -r time out",
            // The same, and the file of `tee` keeps a name.
            "ln out kept; rm out; : > out",
            "rm out; echo other > out",
            // Another file at the same path, with one byte.
            "rm out; printf x > out",
        ] {
            let asked = same_as_without_a_file_writer(
                "changed",
                &format!(
                    "{{ sleep 0.05; {change}; echo late; }} | tee out > /dev/null; \
                     cat out; echo $?; ls; cat ./* | cksum"
                ),
            );
            assert_eq!(asked, [], "{change}");
        }
    }

    /// `tee` itself can write to the file through another descriptor. Here its message goes
    /// to the end of the file before its input comes.
    #[test]
    fn tee_writes_the_file_itself_when_its_own_message_is_in_the_file() {
        let asked = same_as_without_a_file_writer(
            "own-message",
            "echo hi | tee out missing/x 2>> out; cat out",
        );
        assert_eq!(asked, []);
    }

    /// `tee` can also write to the file through another descriptor after it has taken bytes
    /// for it: another operand, or a message on a standard error that adds to the same file.
    /// Such a write must come after those bytes. So `tee` writes them to the file first.
    #[test]
    fn tee_writes_the_file_itself_before_it_writes_to_it_through_another_descriptor() {
        for script in [
            "echo hi | tee out /dev/stderr 2>> out; cat out",
            "echo hi | tee out /dev/fd/3 3< f 2>> out; cat out",
            // The standard input is not a regular file, and a write to it fails.
            "echo hi | tee out /dev/fd/0 2>> out; cat out",
        ] {
            let asked = same_as_without_a_file_writer("own-write", script);
            assert_eq!(asked, [], "{script}");
        }
    }

    /// A standard output that is a regular file does not stop the use of the file writer for
    /// input that comes in one piece: `tee` has taken nothing when it writes that piece.
    #[test]
    fn tee_gives_one_piece_to_the_file_writer_when_its_standard_output_is_a_file() {
        let asked =
            same_as_without_a_file_writer("to-file", "echo hi | tee out > copy; cat out copy");
        assert_eq!(asked.len(), 1);
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

mod edit {
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    use crate::commands::FileEditor;

    /// What the file editor of a test does with a file.
    #[derive(Clone, Copy)]
    enum Answer {
        /// Writes the new content in upper case, so that a test can tell it from the work of
        /// the command.
        UpperCase,
        /// Writes the new content as it is.
        Exact,
        /// Writes nothing and refuses: the command must replace the file itself.
        Refuse,
        /// Writes other bytes and then refuses, as a change that failed part of the way.
        Spoil,
        /// Writes one byte less than the new content and says that it changed the file.
        TooShort,
    }

    /// One change that the file editor was asked for: the path, the old and the new content.
    type Asked = (PathBuf, String, String);

    struct Editor {
        answer: Answer,
        max_len: usize,
        asked: Mutex<Vec<Asked>>,
    }

    impl FileEditor for Editor {
        fn max_len(&self) -> usize {
            self.max_len
        }

        fn edit(&self, path: &Path, old: &[u8], new: &[u8]) -> bool {
            self.asked.lock().unwrap().push((
                path.to_owned(),
                String::from_utf8_lossy(old).into_owned(),
                String::from_utf8_lossy(new).into_owned(),
            ));
            match self.answer {
                Answer::UpperCase => std::fs::write(path, new.to_ascii_uppercase()).is_ok(),
                Answer::Exact => std::fs::write(path, new).is_ok(),
                Answer::Refuse => false,
                Answer::Spoil => {
                    let _ = std::fs::write(path, "other content, and more of it");
                    false
                }
                Answer::TooShort => std::fs::write(path, &new[..new.len() - 1]).is_ok(),
            }
        }
    }

    /// The outcome of a script, and each change that the file editor was asked for.
    struct Outcome {
        status: u8,
        stdout: String,
        stderr: String,
        asked: Vec<Asked>,
    }

    /// Runs `script` in a fresh session with a file editor that takes `max_len` bytes and does
    /// `answer`.
    fn run_limited(script: &str, answer: Answer, max_len: usize) -> Outcome {
        let editor = Arc::new(Editor {
            answer,
            max_len,
            asked: Mutex::default(),
        });
        let (status, stdout, stderr) = super::run(script, |session| {
            session.forward_file_edits(editor.clone());
        });
        let asked = editor.asked.lock().unwrap().clone();
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
        super::directory("edit", name)
    }

    fn content(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// The names in `directory`, in order.
    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Runs `script` in a new directory `name` two times: with no file editor, and with one
    /// that writes the new content as it is. Asserts that the status, the output and what
    /// `show` prints after the script are the same. Returns what the file editor was asked for.
    fn same_as_without_a_file_editor(name: &str, script: &str, show: &str) -> Vec<Asked> {
        let script = |directory: &Path| format!("cd {}; {script}; {show}", directory.display());
        let local = super::run(&script(&directory(name)), |_| ());
        let outcome = run(&script(&directory(name)), Answer::Exact);
        assert_eq!((outcome.status, outcome.stdout, outcome.stderr), local);
        outcome.asked
    }

    /// Prints each name in the working directory with what it is and what it has.
    const SHOW: &str = "for name in * .[!.]*; do [ -e \"$name\" ] || [ -L \"$name\" ] || continue; \
                        [ -L \"$name\" ] && echo \"$name is a link\"; echo \"$name:\"; \
                        cat \"$name\"; done";

    #[test]
    fn sed_gives_an_edit_in_place_to_the_file_editor() {
        let directory = directory("regular");
        let outcome = run(
            &format!("sed -i s/one/1/ {}/f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(
            (
                outcome.status,
                outcome.stdout.as_str(),
                outcome.stderr.as_str()
            ),
            (0, "", "")
        );
        assert_eq!(
            outcome.asked,
            [(
                directory.join("f"),
                "one\ntwo\n".to_owned(),
                "1\ntwo\n".to_owned()
            )]
        );
        assert_eq!(content(&directory.join("f")), "1\nTWO\n");
        assert_eq!(names(&directory), ["f"]);
    }

    #[test]
    fn sed_asks_the_file_editor_with_the_absolute_path_of_a_relative_operand() {
        let directory = directory("relative");
        let outcome = run(
            &format!(
                "cd {}; sed -i -e s/one/1/ -e s/two/2/ f",
                directory.display()
            ),
            Answer::UpperCase,
        );
        assert_eq!(
            outcome.asked,
            [(
                directory.join("f"),
                "one\ntwo\n".to_owned(),
                "1\n2\n".to_owned()
            )]
        );
    }

    #[test]
    fn sed_gives_each_file_operand_to_the_file_editor_in_order() {
        let directory = directory("several");
        std::fs::write(directory.join("g"), "one more\n").unwrap();
        let outcome = run(
            &format!("cd {}; sed -i s/one/1/ g f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(outcome.status, 0);
        assert_eq!(
            outcome.asked,
            [
                (
                    directory.join("g"),
                    "one more\n".to_owned(),
                    "1 more\n".to_owned()
                ),
                (
                    directory.join("f"),
                    "one\ntwo\n".to_owned(),
                    "1\ntwo\n".to_owned()
                )
            ]
        );
        assert_eq!(content(&directory.join("g")), "1 MORE\n");
        assert_eq!(names(&directory), ["f", "g"]);
    }

    /// `sed -i` writes a file that it does not change, and so does the file editor.
    #[test]
    fn sed_gives_a_file_with_no_change_to_the_file_editor() {
        let directory = directory("unchanged");
        let outcome = run(
            &format!("cd {}; sed -i s/none/1/ f", directory.display()),
            Answer::UpperCase,
        );
        assert_eq!(
            outcome.asked,
            [(
                directory.join("f"),
                "one\ntwo\n".to_owned(),
                "one\ntwo\n".to_owned()
            )]
        );
        assert_eq!(content(&directory.join("f")), "ONE\nTWO\n");
    }

    #[test]
    fn sed_replaces_the_file_itself_when_the_file_editor_refuses() {
        let directory = directory("refuse");
        let outcome = run(
            &format!("cd {}; sed -i s/one/1/ f", directory.display()),
            Answer::Refuse,
        );
        assert_eq!((outcome.status, outcome.stderr.as_str()), (0, ""));
        assert_eq!(outcome.asked.len(), 1);
        assert_eq!(content(&directory.join("f")), "1\ntwo\n");
        assert_eq!(names(&directory), ["f"]);
    }

    #[test]
    fn sed_replaces_the_file_itself_when_the_file_editor_left_other_content() {
        let directory = directory("spoil");
        let outcome = run(
            &format!("cd {}; sed -i s/one/1/ f", directory.display()),
            Answer::Spoil,
        );
        assert_eq!((outcome.status, outcome.stderr.as_str()), (0, ""));
        assert_eq!(outcome.asked.len(), 1);
        assert_eq!(content(&directory.join("f")), "1\ntwo\n");
        assert_eq!(names(&directory), ["f"]);
    }

    #[test]
    fn sed_replaces_the_file_itself_when_the_file_editor_wrote_another_length() {
        let directory = directory("length");
        let outcome = run(
            &format!("cd {}; sed -i s/one/1/ f", directory.display()),
            Answer::TooShort,
        );
        assert_eq!((outcome.status, outcome.stderr.as_str()), (0, ""));
        assert_eq!(outcome.asked.len(), 1);
        assert_eq!(content(&directory.join("f")), "1\ntwo\n");
        assert_eq!(names(&directory), ["f"]);
    }

    #[test]
    fn sed_gives_the_file_editor_content_up_to_its_limit() {
        let directory = directory("limit");
        let script = format!("cd {}; sed -i s/one/1/ f; cat f", directory.display());
        // The file has 8 bytes before the change and 6 bytes after it.
        let outcome = run_limited(&script, Answer::UpperCase, 8);
        assert_eq!(outcome.asked.len(), 1);
        std::fs::write(directory.join("f"), "one\ntwo\n").unwrap();
        let outcome = run_limited(&script, Answer::UpperCase, 7);
        assert_eq!(outcome.asked, []);
        assert_eq!((outcome.status, outcome.stdout.as_str()), (0, "1\ntwo\n"));
    }

    /// The file editor changes the file that is there. `sed -i` with a suffix makes that file
    /// the copy, and puts a new file in its place.
    #[test]
    fn sed_does_not_ask_the_file_editor_when_it_keeps_a_copy() {
        for copy in ["", "echo an older copy > f.bak; "] {
            let asked = same_as_without_a_file_editor(
                "copy",
                &format!("{copy}sed -i.bak s/one/1/ f; echo $?"),
                SHOW,
            );
            assert_eq!(asked, [], "{copy}");
        }
    }

    /// `sed -i` puts a regular file in the place of a symbolic link. The file editor writes
    /// through the link.
    #[test]
    fn sed_does_not_ask_the_file_editor_for_a_symbolic_link() {
        let asked = same_as_without_a_file_editor(
            "link",
            "ln -s f link; sed -i s/one/1/ link; echo $?",
            SHOW,
        );
        assert_eq!(asked, []);
    }

    #[test]
    fn sed_asks_the_file_editor_for_the_file_of_a_link_that_it_follows() {
        let directory = directory("follow");
        let outcome = run(
            &format!(
                "cd {}; ln -s f link; sed -i --follow-symlinks s/one/1/ link; echo $?; {SHOW}",
                directory.display()
            ),
            Answer::UpperCase,
        );
        assert_eq!(
            outcome.asked,
            [(
                directory.join("f"),
                "one\ntwo\n".to_owned(),
                "1\ntwo\n".to_owned()
            )]
        );
        assert_eq!(
            outcome.stdout,
            "0\nf:\n1\nTWO\nlink is a link\nlink:\n1\nTWO\n"
        );
    }

    /// `sed -i` leaves the other names of a file with the old content. The file editor changes
    /// the content under each name.
    #[test]
    fn sed_does_not_ask_the_file_editor_for_a_file_with_another_name() {
        let asked = same_as_without_a_file_editor(
            "hard-link",
            "ln f other; sed -i s/one/1/ f; echo $?",
            SHOW,
        );
        assert_eq!(asked, []);
    }

    /// A descriptor that the script has open on the file keeps the old content after `sed -i`.
    /// The file editor changes what that descriptor reads.
    #[test]
    fn sed_does_not_ask_the_file_editor_for_a_file_that_the_script_has_open() {
        let asked = same_as_without_a_file_editor(
            "open",
            "exec 3< f; sed -i s/one/1/ f; echo $?; cat <&3; exec 3<&-",
            SHOW,
        );
        assert_eq!(asked, []);
        // The first `sed -i` leaves the descriptor of the loop on the old file. So the second
        // one has a file that the script does not have open.
        let asked = same_as_without_a_file_editor(
            "open-loop",
            "while read -r word; do sed -i \"s/$word/<$word>/\" f; done < f",
            SHOW,
        );
        assert_eq!(
            asked,
            [(
                Path::new("/tmp/bash-forwarded/edit/open-loop/f").to_owned(),
                "<one>\ntwo\n".to_owned(),
                "<one>\n<two>\n".to_owned()
            )]
        );
    }

    /// A descriptor on another file, and one that the script has closed, do not matter.
    #[test]
    fn sed_asks_the_file_editor_when_the_script_has_another_file_open() {
        let asked = same_as_without_a_file_editor(
            "open-other",
            "echo other > g; exec 3< g; exec 4< f; exec 4<&-; sed -i s/one/1/ f; cat <&3",
            SHOW,
        );
        assert_eq!(asked.len(), 1);
    }

    /// A script can write to a file (`w`) or read lines from a file (`R`) that `sed` also
    /// edits in place. `sed` has such a file open, so it replaces the file itself.
    #[test]
    fn sed_does_not_ask_the_file_editor_for_a_file_that_its_script_has_open() {
        for script in [
            "sed -i 'w g' f g",
            "sed -i 'R g' g f",
            "sed -i -e 's/one/1/w g' f g",
        ] {
            let asked = same_as_without_a_file_editor(
                "script-file",
                &format!("printf 'p\\nq\\n' > g; {script}; echo $?"),
                SHOW,
            );
            assert!(
                asked.iter().all(|(path, _, _)| !path.ends_with("g")),
                "{script}: {asked:?}"
            );
        }
    }

    #[test]
    fn sed_without_an_edit_in_place_does_not_ask_the_file_editor() {
        let asked = same_as_without_a_file_editor(
            "no-edit",
            "sed s/one/1/ f; sed -n -s 1p f f; sed -i s/one/1/ missing; echo $?; mv f g",
            SHOW,
        );
        assert_eq!(asked, []);
    }
}
