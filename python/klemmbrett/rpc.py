"""The Varlink client for the running daemon, from the `klemmbrett-rpc`
package. Outside a klemmbrett configuration, install and import that
package (`klemmbrett_rpc`) on its own; it needs only the standard library."""

from klemmbrett_rpc import INTERFACE, AsyncClient, Client, VarlinkError, socket_path

__all__ = ["INTERFACE", "AsyncClient", "Client", "VarlinkError", "socket_path"]
