<script lang="ts">
  // Sidebar-footer toggle (tech-gui.md §5.1): one press flips light↔dark. Sun while
  // dark (press → light), moon while light. Icon-only, so it survives collapse.
  import { t } from '$lib/i18n';
  import { theme } from '$lib/stores/theme';
  import { Button, Icon } from '$lib/theme';
</script>

<Button
  variant="icon"
  title={$theme === 'dark' ? $t('theme.toLight') : $t('theme.toDark')}
  onclick={() => theme.toggle()}
>
  <Icon name={$theme === 'dark' ? 'sun' : 'moon'} />
</Button>
