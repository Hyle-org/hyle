use std::any::type_name;

pub trait Pick<T> {
    fn get(&self) -> &T;
    fn get_mut(&mut self) -> &mut T;
    /// # Safety
    /// This function is unsafe because it returns a raw pointer to the inner value.
    /// This is intended for splitting borrows.
    unsafe fn splitting_get_mut(&mut self) -> *mut T;
}

pub fn type_name_of_variable<T>(_: &T) -> &'static str {
    type_name::<T>()
}

/// This creates a struct that provides Pick for each type in the initial list,
/// allowing generic code to access the inner values.
/// Only one value of each type can be stored in the struct.
/// (Somewhat similar to frunk HList or a fancier named tuple).
macro_rules! static_type_map {
    // Basic syntax - similar to a named tuple.
    // This just reformats to the recursive form
    (
        $(#[$meta:meta])*
        $pub:vis struct $name:ident ($($t1:ty,)+);
    ) => {
        use paste::paste;
        paste! {
            // Wrap it in a module to make the member names private
            mod [<$name:snake>] {
                use super::*;
                use paste::paste;
                static_type_map! {
                    $(#[$meta])*
                    ha: $pub struct $name ($($t1,)+) {}
                }
            }
            #[doc(inline)]
            $pub use [<$name:snake>]::$name;
        }
    };
    // Recursive case - we append to $index every time, processing one type.
    (
        $(#[$meta:meta])*
        $index:ident: $pub:vis struct $name:ident ($t1:ty, $($t2:ty,)*) { $($idx:ident: $t3:ty,)* }
    ) => {
        paste! {
            static_type_map! {
                $(#[$meta])*
                [<ha $index>]: $pub struct $name ( $($t2,)* ) {
                    $($idx: $t3,)*
                    $index: $t1,
                }
            }
        }
        impl $crate::utils::static_type_map::Pick<$t1> for $name {
            fn get(&self) -> &$t1 {
                &self.$index
            }
            fn get_mut(&mut self) -> &mut $t1 {
                &mut self.$index
            }
            unsafe fn splitting_get_mut(&mut self) -> *mut $t1 {
                &mut self.$index as *mut $t1
            }
        }
    };
    // End state - just output the struct and the new function (which works like the named tuple constructor)
    (
        $(#[$meta:meta])*
        $index:ident: $pub:vis struct $name:ident ( ) { $($idx:ident: $t3:ty,)+ }
    ) => {
        $(#[$meta])*
        pub(super) struct $name {
            $($idx: $t3,)+
        }
        impl $name {
            #[allow(clippy::too_many_arguments)]
            pub fn new($($idx: $t3,)*) -> Self {
                Self {
                    $($idx,)*
                }
            }
        }
    };
}
pub(crate) use static_type_map;
