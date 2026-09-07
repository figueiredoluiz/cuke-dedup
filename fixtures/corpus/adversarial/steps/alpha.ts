function handler() {
  performAlphaOperation();
}

function notImplemented() {
  throw new Error('pending');
}

Given('the alpha operation runs', handler);
Given('the pending alpha operation runs', notImplemented);
