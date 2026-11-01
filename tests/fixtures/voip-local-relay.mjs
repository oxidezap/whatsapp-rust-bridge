// Synthetic opaque relay shared by installed-package media fixtures. No sockets or auth.
export function createLocalRelay(onRtp) {
  const endpoints = [undefined, undefined];
  return {
    transport(index) {
      return {
        async connect(_params, events) {
          endpoints[index] = events;
          return {
            send(packet) {
              const copy = packet.slice();
              if (copy[0] === 0 && copy[1] === 3) {
                const response = copy.slice(0, 20);
                response[0] = 1; response[1] = 3; response[2] = 0; response[3] = 0;
                queueMicrotask(() => endpoints[index]?.onPacket(response));
              } else if ((copy[0] & 0xc0) === 0x80) {
                onRtp(index, copy);
                queueMicrotask(() => endpoints[1 - index]?.onPacket(copy));
              }
            },
            reconnect() {},
            close() { endpoints[index] = undefined; },
          };
        },
      };
    },
    get activeConnections() { return endpoints.filter(Boolean).length; },
  };
}
