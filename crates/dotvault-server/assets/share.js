// dotvault share landing page.
//
// Reads the share key from location.hash and shows the CLI command. The key
// stays in this tab: nothing here makes a network request (the page's CSP
// forbids it), and the fragment is never sent to the server by the browser.
(function () {
  'use strict';

  var ID_RE = /^[A-Za-z0-9_-]{22}$/;
  var KEY_RE = /^[A-Za-z0-9_-]{43}$/;

  function $(id) {
    return document.getElementById(id);
  }

  function showProblem(text) {
    $('ready').classList.add('hidden');
    $('problem').classList.remove('hidden');
    $('problem-text').textContent = text;
  }

  var segments = location.pathname.split('/').filter(Boolean);
  var id = segments.length === 2 && segments[0] === 's' ? segments[1] : '';
  var key = location.hash.replace(/^#/, '');

  if (!ID_RE.test(id)) {
    showProblem('This link looks malformed (the share id is not valid). Ask the sender for a new one.');
    return;
  }
  if (!key) {
    showProblem('This link is missing its key (the part after #). Ask the sender to send the complete link or code.');
    return;
  }
  if (!KEY_RE.test(key)) {
    showProblem('This link looks malformed (the key is not valid). Make sure the whole link was copied.');
    return;
  }

  var command = 'dotvault open dv1.' + id + '.' + key;
  $('cmd').textContent = command;
  $('alt').textContent = 'dotvault open "' + location.origin + '/s/' + id + '#' + key + '"';

  var button = $('copy');
  var status = $('copied');

  function done(ok) {
    status.textContent = ok ? ' Copied.' : ' Press Ctrl+C to copy the command above.';
    window.setTimeout(function () {
      status.textContent = '';
    }, 3000);
  }

  function legacyCopy() {
    var range = document.createRange();
    range.selectNodeContents($('cmd'));
    var selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    var ok = false;
    try {
      ok = document.execCommand('copy');
    } catch (e) {
      ok = false;
    }
    done(ok);
  }

  button.addEventListener('click', function () {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(command).then(
        function () {
          done(true);
        },
        legacyCopy
      );
    } else {
      legacyCopy();
    }
  });
})();
