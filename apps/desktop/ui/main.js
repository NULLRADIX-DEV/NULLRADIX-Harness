// the shell passes a failure as ?error=..., shown instead of the progress text
const error = new URLSearchParams(location.search).get('error');
const message = document.getElementById('message');
if (error && message) message.textContent = error;
