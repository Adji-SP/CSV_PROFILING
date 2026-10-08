const form = document.getElementById("login-form");
const submit = document.getElementById("login-submit");
const message = document.getElementById("login-error");
form.addEventListener("submit", async event => {
  event.preventDefault();
  submit.disabled = true;
  message.textContent = "";
  try {
    const response = await fetch("/auth/login", {
      method: "POST", credentials: "same-origin",
      headers: {"Content-Type": "application/json"},
      body: JSON.stringify({username: form.username.value.trim(), password: form.password.value,
                            code: form.code.value.replace(/\s/g, "")}),
    });
    const body = await response.json();
    if (!response.ok) throw new Error(body.error?.message || "Could not log in. Please try again.");
    window.location.replace("/");
  } catch (error) {
    message.textContent = error instanceof TypeError ? "Cannot reach the dashboard. Please try again." : error.message;
    form.code.value = "";
    submit.disabled = false;
  }
});
