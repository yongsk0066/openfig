(() => {
  const nonce = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  let resolve;
  let fatalPending = false;
  const ready = new Promise((done) => { resolve = done; });
  const sendToHost = window.ipc.postMessage.bind(window.ipc);
  window.figRuntime = {
    nonce, epoch: undefined, ready,
    fatal() { fatalPending = true; if (this.epoch !== undefined) sendToHost(`fig-runtime:fatal:${this.epoch}`); },
    hello() { sendToHost(`fig-runtime:hello:${nonce}`); },
    bind(documentNonce, epoch) {
      if (documentNonce !== nonce || this.epoch !== undefined) return;
      this.epoch = epoch;
      resolve(epoch);
      if (fatalPending) this.fatal();
    },
  };
  window.figRuntime.hello();
})();
