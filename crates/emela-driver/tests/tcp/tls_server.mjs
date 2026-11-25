// A TLS echo server for tests/tcp.rs, with the self-signed certificate of `localhost` in this
// directory. Prints its port on a line, then echoes what each client sends until the client
// closes. Ends when its standard input closes.
import { readFileSync } from "node:fs";
import { createServer } from "node:tls";

const server = createServer(
  {
    key: readFileSync(new URL("key.pem", import.meta.url)),
    cert: readFileSync(new URL("cert.pem", import.meta.url)),
  },
  (socket) => {
    socket.on("error", () => {});
    socket.pipe(socket);
  },
);
server.on("tlsClientError", () => {});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
process.stdin.on("end", () => process.exit(0));
process.stdin.resume();
