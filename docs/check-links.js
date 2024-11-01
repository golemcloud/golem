const glob = require("glob")
const { exec } = require("child_process")

glob("**/rib.mdx", (err, files) => {
  if (err) {
    console.error("Error finding files:", err)
    process.exit(1)
  }

  files.forEach(file => {
    exec(`markdown-link-check ${file}`, (err, stdout, stderr) => {
      if (err) {
        console.error(`Error checking links in ${file}:`, stderr || stdout)
        console.log(stdout)
        console.log(stderr)
        console.log(err)
        process.exit(1)
      } else {
        console.log(`Checked links in ${file}:`, stdout)
      }
    })
  })
})
