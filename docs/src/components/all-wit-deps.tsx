import { FileTree } from "nextra/components"

export const AllWitDeps = () => {
  return (
    <FileTree>
      <FileTree.Folder name="src" open={false}>
        {null}
      </FileTree.Folder>

      <FileTree.Folder name="wit" defaultOpen>
        <FileTree.File name="main.wit" />
        <FileTree.Folder name="deps" defaultOpen>
          <FileTree.Folder name="blobstore">
            <FileTree.File name="blobstore.wit" />
            <FileTree.File name="container.wit" />
            <FileTree.File name="types.wit" />
            <FileTree.File name="world.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="cli">
            <FileTree.File name="command.wit" />
            <FileTree.File name="environment.wit" />
            <FileTree.File name="exit.wit" />
            <FileTree.File name="imports.wit" />
            <FileTree.File name="run.wit" />
            <FileTree.File name="stdio.wit" />
            <FileTree.File name="terminal.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="clocks">
            <FileTree.File name="monotonic-clock.wit" />
            <FileTree.File name="wall-clock.wit" />
            <FileTree.File name="world.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="filesystem">
            <FileTree.File name="preopens.wit" />
            <FileTree.File name="types.wit" />
            <FileTree.File name="world.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="golem">
            <FileTree.File name="golem-host.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="http">
            <FileTree.File name="handler.wit" />
            <FileTree.File name="proxy.wit" />
            <FileTree.File name="types.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="io">
            <FileTree.File name="error.wit" />
            <FileTree.File name="poll.wit" />
            <FileTree.File name="streams.wit" />
            <FileTree.File name="world.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="keyvalue">
            <FileTree.File name="atomic.wit" />
            <FileTree.File name="caching.wit" />
            <FileTree.File name="error.wit" />
            <FileTree.File name="eventual-batch.wit" />
            <FileTree.File name="eventual.wit" />
            <FileTree.File name="handle-watch.wit" />
            <FileTree.File name="types.wit" />
            <FileTree.File name="world.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="logging">
            <FileTree.File name="logging.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="random">
            <FileTree.File name="insecure-seed.wit" />
            <FileTree.File name="insecure.wit" />
            <FileTree.File name="random.wit" />
            <FileTree.File name="world.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="sockets">
            <FileTree.File name="instance-network.wit" />
            <FileTree.File name="ip-name-lookup.wit" />
            <FileTree.File name="network.wit" />
            <FileTree.File name="tcp-create-socket.wit" />
            <FileTree.File name="tcp.wit" />
            <FileTree.File name="udp-create-socket.wit" />
            <FileTree.File name="udp.wit" />
            <FileTree.File name="world.wit" />
          </FileTree.Folder>
          <FileTree.Folder name="wasm-rpc">
            <FileTree.File name="wasm-rpc.wit" />
          </FileTree.Folder>
        </FileTree.Folder>
      </FileTree.Folder>
    </FileTree>
  )
}
