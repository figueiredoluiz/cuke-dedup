function handler() {
  performUnrelatedBetaOperation();
}

function notImplemented() {
  return 'pending';
}

When('the beta operation runs', handler);
When('the pending beta operation runs', notImplemented);
