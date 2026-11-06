import { useComputedColorScheme } from '@mantine/core';
import logoBlack from '../assets/chatgpt-logo-black.svg';
import logoWhite from '../assets/chatgpt-logo-white.svg';
import classes from './ContinueWithChatGPT.module.css';

interface Props {
  onClick: () => void;
  loading?: boolean;
}

/** Black button with the white logo in light mode, white with the black logo in dark mode. */
export function ContinueWithChatGPT({ onClick, loading = false }: Props) {
  const dark = useComputedColorScheme('light') === 'dark';
  return (
    <button
      type="button"
      className={`${classes.button} ${dark ? classes.white : classes.black}`}
      onClick={onClick}
      disabled={loading}
      aria-busy={loading || undefined}
    >
      <img className={classes.logo} src={dark ? logoBlack : logoWhite} alt="" />
      <span>{loading ? 'Opening ChatGPT…' : 'Continue with ChatGPT'}</span>
    </button>
  );
}
