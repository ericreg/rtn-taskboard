<script lang="ts">
  import { ArrowRight, LayoutDashboard, Check, LockKeyhole } from '@lucide/svelte';
  import { onMount } from 'svelte';
  import { api, errorMessage, go, setCsrf } from '../lib/api';
  import type { Session } from '../lib/types';
  let { route, onsignedin }: { route: string; onsignedin: () => void } = $props();
  const params = new URLSearchParams(location.hash.slice(1));
  let email = $state(params.get('email') || ''); let password = $state(''); let name = $state('');
  let token = $state(params.get('token') || ''); let error = $state(''); let success = $state(''); let busy = $state(false); let bootstrap = $state(false);
  const mode = $derived(route === '/register' ? 'register' : route === '/reset' ? 'reset' : 'login');
  onMount(() => { void api<{ bootstrap_required: boolean }>('/auth/setup').then(v => bootstrap = v.bootstrap_required).catch(() => {}); });
  async function submit(event: SubmitEvent) {
    event.preventDefault(); error = ''; success = ''; busy = true;
    try {
      if (mode === 'register') { await api('/auth/register', 'POST', { email, name, password, token }); success = 'Your account is ready. You can now sign in.'; password = ''; }
      else if (mode === 'reset') { await api('/auth/reset-password', 'POST', { token, password }); success = 'Password updated. You can now sign in.'; password = ''; }
      else { const session = await api<Session>('/auth/login', 'POST', { email, password }); setCsrf(session.csrf); onsignedin(); }
    } catch (e) { error = errorMessage(e); } finally { busy = false; }
  }
</script>
<div class="auth-page">
  <div class="auth-story">
    <a class="brand" href="/" onclick={(e) => go(e, '/')}><span class="brand-icon"><LayoutDashboard size={21}/></span>taskboard<span class="brand-dot">.</span></a>
    <div class="auth-story-content"><span class="eyebrow">A LITTLE LESS SCATTERED</span><h1>Good work.<br/>All in one place.</h1><p>A shared home for your team's projects, ideas, and the next thing to get done.</p>
      <div class="auth-illustration" aria-hidden="true"><div class="illustration-heading"><span class="project-square"></span>Website refresh<span class="tiny-tag">3 tasks</span></div><div class="illustration-task"><span class="mini-check"><Check size={12}/></span>Give every idea a place<span class="avatar small">JL</span></div><div class="illustration-task"><span class="mini-progress"></span>Keep the team in the loop<span class="tiny-tag warm">In progress</span></div><div class="illustration-task"><span class="mini-empty"></span>Make room for what's next<span class="tiny-tag">To do</span></div><div class="illustration-footer"><span class="avatar small purple">TB</span><span>Less tracking. More doing.</span></div></div>
    </div><div class="auth-story-footer">Built for your team. Kept on your terms.</div>
  </div>
  <main class="auth-form-side"><div class="auth-form-wrap">
    <div class="auth-lock"><LockKeyhole size={22}/></div>
    <span class="eyebrow">YOUR TEAM'S WORKSPACE</span>
    <h2>{mode === 'register' ? 'Make yourself at home.' : mode === 'reset' ? 'A fresh start.' : 'Welcome back.'}</h2>
    <p class="muted">{mode === 'register' ? 'Use your invitation to join Taskboard.' : mode === 'reset' ? 'Choose a new password for your account.' : 'Sign in to pick up where you left off.'}</p>
    {#if bootstrap}<div class="notice">Taskboard is ready for setup. Your operator can create the first editor with the container's <code>bootstrap</code> command in the README.</div>{/if}
    {#if success}<div class="notice success" role="status">{success} <a href="/login" onclick={(e) => go(e, '/login')}>Sign in <ArrowRight size={14}/></a></div>
    {:else}<form onsubmit={submit}>
      {#if mode === 'register'}<label for="auth-name">Your name</label><input id="auth-name" bind:value={name} autocomplete="name" placeholder="Alex Morgan" required maxlength="100"/>{/if}
      {#if mode !== 'reset'}<label for="auth-email">Work email</label><input id="auth-email" type="email" bind:value={email} autocomplete="username" placeholder="you@company.com" required/>{/if}
      {#if mode !== 'login' && !params.get('token')}<label for="auth-token">{mode === 'register' ? 'Invitation' : 'Reset'} token</label><input id="auth-token" bind:value={token} required autocomplete="off"/>{/if}
      <label for="auth-password">{mode === 'login' ? 'Password' : 'Choose a password'}</label><input id="auth-password" type="password" bind:value={password} autocomplete={mode === 'login' ? 'current-password' : 'new-password'} required minlength={mode === 'login' ? undefined : 15} maxlength="1024" placeholder={mode === 'login' ? 'Enter your password' : 'At least 15 characters'}/>
      {#if error}<div class="form-error" role="alert">{error}</div>{/if}
      <button class="button primary auth-submit" disabled={busy} type="submit">{busy ? 'One moment…' : mode === 'register' ? 'Create account' : mode === 'reset' ? 'Update password' : 'Sign in'}<ArrowRight size={17}/></button>
    </form>{/if}
    <div class="auth-help">{#if mode === 'login'}Need an account or a password reset?<br/>Ask a workspace editor for a link.{:else}<a href="/login" onclick={(e) => go(e, '/login')}>Already have an account? Sign in</a>{/if}</div>
    <div class="auth-private"><span class="online-dot"></span>A private workspace for your company</div>
  </div></main>
</div>
