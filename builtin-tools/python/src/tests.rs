use super::runtime::{self, Stdio};
use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

#[derive(Clone, Default)]
struct Capture(Rc<RefCell<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn execute(cwd: &str, args: &[&str], input: &[u8]) -> (i32, Vec<u8>, String) {
    let output = Capture::default();
    let error = Capture::default();
    let streams = Rc::new(RefCell::new(Stdio::new(
        Box::new(io::Cursor::new(input.to_vec())),
        Box::new(output.clone()),
        Box::new(error.clone()),
    )));
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let status = runtime::run(cwd, &args, streams).unwrap();
    (
        status,
        output.0.borrow().clone(),
        String::from_utf8(error.0.borrow().clone()).unwrap(),
    )
}

// Native cwd is process-wide; one contract test exercises all invocations serially.
#[test]
fn python_cli_contract() {
    let root = std::env::temp_dir().join(format!("golem-python-test-{}", std::process::id()));
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    let cwd = root.to_str().unwrap();
    let original = std::env::current_dir().unwrap();
    assert_eq!(
        execute(
            cwd,
            &["-c", "import sys; print(__name__, sys.argv)", "a", "--flag"],
            b""
        ),
        (
            0,
            b"__main__ ['-c', 'a', '--flag']\n".to_vec(),
            String::new()
        )
    );
    assert_eq!(
        execute(
            cwd,
            &[
                "-u",
                "-c",
                "import sys; sys.stdout.buffer.write(sys.stdin.buffer.read()); print('err', file=sys.stderr)"
            ],
            b"\0\xffhi\n"
        ),
        (0, b"\0\xffhi\n".to_vec(), "err\n".into())
    );
    assert_eq!(
        execute(cwd, &["-", "a"], b"import sys; print(sys.argv)\n"),
        (0, b"['-', 'a']\n".to_vec(), String::new())
    );
    assert_eq!(
        execute(cwd, &[], b"print(6 * 7)\n"),
        (0, b"42\n".to_vec(), String::new())
    );
    std::fs::write(root.join("helper.py"), "answer = 17\n").unwrap();
    std::fs::write(root.join("script.py"), "import helper, sys, os\nprint(__name__, os.path.basename(__file__), helper.answer, sys.argv)\nopen('persistent.txt', 'w').write('saved')\nos.chdir('pkg')\n").unwrap();
    assert_eq!(
        execute(cwd, &["script.py", "tail"], b""),
        (
            0,
            b"__main__ script.py 17 ['script.py', 'tail']\n".to_vec(),
            String::new()
        )
    );
    assert_eq!(std::env::current_dir().unwrap(), original);
    assert_eq!(
        std::fs::read_to_string(root.join("persistent.txt")).unwrap(),
        "saved"
    );
    std::fs::write(root.join("pkg/__init__.py"), "").unwrap();
    std::fs::write(
        root.join("pkg/__main__.py"),
        "import sys\nprint(__name__, __package__, sys.argv[1:])\n",
    )
    .unwrap();
    assert_eq!(
        execute(cwd, &["-m", "pkg", "-x"], b""),
        (0, b"__main__ pkg ['-x']\n".to_vec(), String::new())
    );
    for (code, expected, message) in [
        ("None", 0, ""),
        ("7", 7, ""),
        ("-1", 255, ""),
        ("258", 2, ""),
        ("'failure'", 1, "failure\n"),
    ] {
        assert_eq!(
            execute(
                cwd,
                &[
                    "-c",
                    &format!("import sys; print('before'); sys.exit({code})")
                ],
                b""
            ),
            (expected, b"before\n".to_vec(), message.into())
        );
    }
    let (status, out, err) = execute(
        cwd,
        &["-c", "print('partial'); raise ValueError('broken')"],
        b"",
    );
    assert_eq!(status, 1);
    assert_eq!(out, b"partial\n");
    assert!(
        err.contains("ValueError:") && err.contains("broken"),
        "{err}"
    );
    let (status, _, err) = execute(cwd, &["-c", "if :"], b"");
    assert_eq!(status, 1);
    assert!(err.contains("SyntaxError"), "{err}");
    for (code, expected) in [
        (
            "import sys, atexit; sys.stdout.keepalive = sys.stdout; atexit.register(print, 'shutdown')",
            "shutdown\n",
        ),
        (
            "import sys, io, atexit; original = sys.stdout; original.keepalive = original; print('shutdown'); sys.stdout = sys.__stdout__ = io.StringIO(); atexit.register(lambda: original.write('atexit'))",
            "shutdown\natexit",
        ),
    ] {
        let (status, out, err) = execute(cwd, &["-c", code], b"");
        assert_eq!(status, 0);
        assert_eq!(out, expected.as_bytes());
        assert!(err.is_empty());
    }
    let (status, out, err) = execute(
        cwd,
        &[
            "-c",
            "import atexit, sys\ndef fail():\n del sys.unraisablehook\n raise RuntimeError('shutdown')\natexit.register(fail)",
        ],
        b"",
    );
    assert_eq!(status, 0);
    assert!(out.is_empty());
    assert!(err.contains("RuntimeError"), "{err}");
    assert_eq!(
        execute(
            cwd,
            &[
                "-c",
                "import sys, io, threading; original = sys.stdout; original.keepalive = original; sys.stdout = sys.__stdout__ = io.StringIO(); threading._register_atexit(original.write, 'shutdown\\n')"
            ],
            b""
        ),
        (0, b"shutdown\n".to_vec(), String::new())
    );
    assert_eq!(
        execute(
            cwd,
            &["-c", "import sys; print('helper' in sys.modules)"],
            b""
        ),
        (0, b"False\n".to_vec(), String::new())
    );
    assert_eq!(
        execute(cwd, &["-O", "-c", "assert False; print(__debug__)"], b""),
        (0, b"False\n".to_vec(), String::new())
    );
    assert_eq!(
        execute(cwd, &["-P", "-c", "import sys; print('' in sys.path)"], b""),
        (0, b"False\n".to_vec(), String::new())
    );
    assert_eq!(
        execute(
            cwd,
            &[
                "-c",
                "import json, math; print(json.dumps({'n': math.factorial(5)}))"
            ],
            b""
        ),
        (0, b"{\"n\": 120}\n".to_vec(), String::new())
    );
    for args in [&["-i"][..], &["-c"][..], &["--unsupported"][..]] {
        assert_eq!(execute(cwd, args, b"").0, 2);
    }
    assert_eq!(
        execute(
            "/does-not-exist-golem-python",
            &["-c", "open('unexpected', 'w')"],
            b""
        )
        .0,
        2
    );
    assert!(!root.join("unexpected").exists());
    assert_eq!(std::env::current_dir().unwrap(), original);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tool_metadata_has_cli_streams_and_filesystem_requirement() {
    use golem_rust::agentic::get_all_tools;
    let tools = get_all_tools();
    let tool = tools
        .iter()
        .find(|t| t.commands.nodes[0].name == "python")
        .unwrap();
    assert!(tool.requires_filesystem);
    let body = tool.commands.nodes[0].body.as_ref().unwrap();
    assert!(body.stdin.is_some());
    assert!(body.stdout.is_some());
    assert!(body.stderr.is_some());
}
