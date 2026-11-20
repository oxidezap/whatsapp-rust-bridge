// Synthetic opaque relay shared by installed-package media fixtures. No sockets or auth.
export function createLocalRelay(onRtp) {
  const endpoints = [undefined, undefined];
  return {
    transport(index) {
      return {
        async connect(_params, events) {
          const endpoint = { events, closed: false };
          endpoints[index] = endpoint;
          return {
            send(packet) {
              if (endpoint.closed) return;
              const copy = packet.slice();
              if (copy[0] === 0 && copy[1] === 3) {
                const response = copy.slice(0, 20);
                response[0] = 1; response[1] = 3; response[2] = 0; response[3] = 0;
                queueMicrotask(() => {
                  if (!endpoint.closed && endpoints[index] === endpoint) events.onPacket(response);
                });
              } else if ((copy[0] & 0xc0) === 0x80) {
                onRtp(index, copy);
                const target = endpoints[1 - index];
                queueMicrotask(() => {
                  if (!endpoint.closed && target && !target.closed && endpoints[1 - index] === target)
                    target.events.onPacket(copy);
                });
              }
            },
            close() {
              endpoint.closed = true;
              if (endpoints[index] === endpoint) endpoints[index] = undefined;
            },
          };
        },
      };
    },
    get activeConnections() { return endpoints.filter(Boolean).length; },
  };
}
